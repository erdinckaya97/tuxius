use crate::i18n::{fill, t};
use crate::private;
use crate::store::config_dir;
use anyhow::{Context, Result, bail};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The key Tuxius generates and installs with a password.
pub const DEFAULT: &str = "tuxius";

pub struct KeyInfo {
    pub name: String,
    pub kind: String,
    pub fingerprint: String,
}

pub fn dir() -> Result<PathBuf> {
    Ok(config_dir()?.join("keys"))
}

pub fn path(name: &str) -> Result<PathBuf> {
    Ok(dir()?.join(name))
}

pub fn pub_of(key: &Path) -> PathBuf {
    let mut p = key.as_os_str().to_owned();
    p.push(".pub");
    p.into()
}

pub fn exists(name: &str) -> bool {
    path(name).map(|p| p.exists()).unwrap_or(false)
}

pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        Err(t().name_empty.into())
    } else if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        || name.starts_with('.')
    {
        Err(t().key_chars.into())
    } else if name.ends_with(".pub") {
        Err(t().key_pub_suffix.into())
    } else if exists(name) {
        Err(fill(t().key_exists, &[name]))
    } else {
        Ok(())
    }
}

fn make_dir() -> Result<PathBuf> {
    let dir = dir()?;
    private::create_dir(&dir)?;
    Ok(dir)
}

fn stderr(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_string()
}

pub fn list() -> Result<Vec<KeyInfo>> {
    let dir = dir()?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut keys = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.ends_with(".pub") || name.starts_with('.') || !path.is_file() {
            continue;
        }
        let (kind, fingerprint) = describe(&path);
        keys.push(KeyInfo {
            name: name.to_string(),
            kind,
            fingerprint,
        });
    }
    keys.sort_by(|a, b| {
        (a.name != DEFAULT)
            .cmp(&(b.name != DEFAULT))
            .then(a.name.cmp(&b.name))
    });
    Ok(keys)
}

/// Key type and fingerprint as reported by `ssh-keygen -l`.
fn describe(path: &Path) -> (String, String) {
    let target = if pub_of(path).exists() {
        pub_of(path)
    } else {
        path.to_path_buf()
    };
    let out = Command::new("ssh-keygen")
        .arg("-lf")
        .arg(&target)
        .stdin(Stdio::null())
        .output();
    let text = match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => return ("?".into(), "passphrase-protected or unreadable".into()),
    };
    let full = text.split_whitespace().nth(1).unwrap_or("?");
    // Enough to tell keys apart without wrapping the table.
    let fingerprint = match full.char_indices().nth(23) {
        Some((cut, _)) => format!("{}…", &full[..cut]),
        None => full.to_string(),
    };
    let kind = text
        .trim()
        .rsplit_once('(')
        .map(|(_, k)| k.trim_end_matches(')').to_lowercase())
        .unwrap_or_else(|| "?".into());
    (kind, fingerprint)
}

pub fn public_key(name: &str) -> Result<String> {
    let pub_path = pub_of(&path(name)?);
    let text = fs::read_to_string(&pub_path)
        .with_context(|| format!("no public key for '{name}' (is it passphrase-protected?)"))?;
    let key = text.trim().to_string();
    if key.contains('\'') || key.split_whitespace().count() < 2 {
        bail!("unexpected public key format in {}", pub_path.display());
    }
    Ok(key)
}

pub fn generate(name: &str) -> Result<PathBuf> {
    make_dir()?;
    let key = path(name)?;
    let comment = format!("tuxius-{name}@{}", hostname());
    let out = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", &comment, "-f"])
        .arg(&key)
        .stdin(Stdio::null())
        .output()
        .context("could not run ssh-keygen (is OpenSSH installed?)")?;
    if !out.status.success() {
        bail!("ssh-keygen failed: {}", stderr(&out.stderr));
    }
    Ok(key)
}

pub fn ensure_default() -> Result<PathBuf> {
    let key = path(DEFAULT)?;
    if key.exists() {
        Ok(key)
    } else {
        generate(DEFAULT)
    }
}

pub fn expand_home(input: &str) -> PathBuf {
    match input.strip_prefix("~/") {
        Some(rest) => dirs::home_dir()
            .map(|h| h.join(rest))
            .unwrap_or_else(|| input.into()),
        None => PathBuf::from(input),
    }
}

/// Copies a private key file into the Tuxius key store.
pub fn import(name: &str, source: &Path) -> Result<()> {
    if let Err(msg) = validate_name(name) {
        bail!(msg);
    }
    let text = fs::read_to_string(source)
        .with_context(|| format!("could not read {}", source.display()))?;
    if !text.contains("PRIVATE KEY") {
        bail!(fill(t().not_private_key, &[&source.display().to_string()]));
    }
    make_dir()?;
    let dest = path(name)?;
    write_private(&dest, text.as_bytes())?;

    let derived = Command::new("ssh-keygen")
        .args(["-y", "-P", "", "-f"])
        .arg(&dest)
        .stdin(Stdio::null())
        .output();
    let pub_text = match derived {
        Ok(o) if o.status.success() => Some(String::from_utf8_lossy(&o.stdout).trim().to_string()),
        _ => fs::read_to_string(pub_of(source))
            .ok()
            .map(|t| t.trim().to_string()),
    };
    if let Some(pub_text) = pub_text.filter(|t| !t.is_empty()) {
        fs::write(pub_of(&dest), format!("{pub_text}\n"))?;
    }
    Ok(())
}

fn write_private(dest: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = private::create_file(dest, true)?;
    file.write_all(bytes)?;
    if !bytes.ends_with(b"\n") {
        file.write_all(b"\n")?;
    }
    Ok(())
}

pub fn remove(name: &str) -> Result<()> {
    let key = path(name)?;
    if !key.exists() {
        bail!(fill(t().no_key_named, &[name]));
    }
    fs::remove_file(&key)?;
    let _ = fs::remove_file(pub_of(&key));
    Ok(())
}

fn hostname() -> String {
    Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "localhost".into())
}
