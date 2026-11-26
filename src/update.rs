use crate::i18n::{fill, t};
use crate::store;
use crate::ui;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const REPO: &str = "erdinckaya97/tuxius";
/// How long a "nothing new" check is trusted before asking GitHub again.
const CHECK_EVERY: u64 = 12 * 60 * 60;

#[derive(Debug, Default, Serialize, Deserialize)]
struct Cache {
    checked: u64,
    latest: String,
}

pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Prints a one-line notice when a newer release is known.
/// `refresh` contacts GitHub if the saved check is old; failures stay quiet.
pub fn announce(refresh: bool) {
    if std::env::var_os("TUXIUS_NO_UPDATE").is_some() || !std::io::stderr().is_terminal() {
        return;
    }
    let cached = load_cache();
    let mut shown = String::new();
    if let Some(cache) = cached.as_ref().filter(|c| is_newer(&c.latest, current())) {
        say_available(&cache.latest);
        shown = cache.latest.clone();
    }
    let stale = cached
        .as_ref()
        .map(|c| now().saturating_sub(c.checked) >= CHECK_EVERY)
        .unwrap_or(true);
    if !refresh || !stale {
        return;
    }
    if let Ok(latest) = fetch_latest() {
        save_cache(&latest);
        if is_newer(&latest, current()) && latest != shown {
            say_available(&latest);
        }
    }
}

pub fn perform() -> Result<()> {
    ui::step(t().update_checking);
    let latest = fetch_latest()?;
    save_cache(&latest);
    let current = current();
    if is_newer(&latest, current) {
        ui::step(&fill(t().update_downloading, &[&latest]));
        install(&latest)?;
        ui::ok(&fill(t().update_done, &[&latest]));
    } else if is_newer(current, &latest) {
        ui::ok(&fill(t().update_ahead, &[current, &latest]));
    } else {
        ui::ok(&fill(t().update_current, &[current]));
    }
    Ok(())
}

fn say_available(version: &str) {
    eprintln!(
        "  {} {}",
        ui::accent("↑"),
        fill(
            t().update_available,
            &[version, &ui::accent("tuxius update")]
        )
    );
}

fn fetch_latest() -> Result<String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = curl_text(&url, 3)?;
    let tag = json_string(&body, "tag_name").context(t().update_failed)?;
    let version = tag.trim().trim_start_matches('v').to_string();
    if parse_version(&version).is_none() {
        bail!(t().update_failed);
    }
    Ok(version)
}

fn install(version: &str) -> Result<()> {
    let asset = asset_name()?;
    let url = format!("https://github.com/{REPO}/releases/download/v{version}/{asset}");
    let tmp = TempDir::new()?;
    let archive = tmp.0.join(&asset);
    curl_file(&url, &archive).context(t().update_failed)?;
    let status = Command::new("tar")
        .args(["-xf"])
        .arg(&archive)
        .arg("-C")
        .arg(&tmp.0)
        .status()
        .context(t().update_failed)?;
    if !status.success() {
        bail!(t().update_failed);
    }
    let bin = find_bin(&tmp.0)?;
    replace_current(&bin)
}

fn asset_name() -> Result<String> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => bail!("{} ({other})", t().update_failed),
    };
    let (os, ext) = match std::env::consts::OS {
        "linux" => ("unknown-linux-musl", "tar.gz"),
        "macos" => ("apple-darwin", "tar.gz"),
        "windows" => ("pc-windows-msvc", "zip"),
        other => bail!(format!("{} ({other})", t().update_failed)),
    };
    Ok(format!("tuxius-{arch}-{os}.{ext}"))
}

fn find_bin(dir: &Path) -> Result<PathBuf> {
    let name = if cfg!(windows) {
        "tuxius.exe"
    } else {
        "tuxius"
    };
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).with_context(|| t().update_failed)?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().and_then(|n| n.to_str()) == Some(name) {
                return Ok(path);
            }
        }
    }
    bail!(t().update_failed)
}

fn replace_current(new_bin: &Path) -> Result<()> {
    let dest = std::env::current_exe().context(t().update_failed)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(new_bin, fs::Permissions::from_mode(0o755));
        if fs::copy(new_bin, &dest).is_ok() {
            let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(0o755));
            return Ok(());
        }
        if !std::io::stdin().is_terminal() {
            bail!(t().update_failed);
        }
        ui::step(&fill(t().update_sudo, &[&dest.display().to_string()]));
        let status = Command::new("sudo")
            .args(["install", "-m", "755"])
            .arg(new_bin)
            .arg(&dest)
            .status()
            .context(t().update_failed)?;
        if status.success() {
            Ok(())
        } else {
            bail!(t().update_failed)
        }
    }
    #[cfg(not(unix))]
    {
        let old = dest.with_extension("old");
        let _ = fs::remove_file(&old);
        fs::rename(&dest, &old).context(t().update_failed)?;
        fs::rename(new_bin, &dest).context(t().update_failed)?;
        Ok(())
    }
}

fn curl_text(url: &str, seconds: u64) -> Result<String> {
    let secs = seconds.to_string();
    let out = Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            &secs,
            "-H",
            "User-Agent: tuxius",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "Cache-Control: no-cache",
            "-H",
            "Pragma: no-cache",
            url,
        ])
        .output()
        .context(t().update_no_curl)?;
    if !out.status.success() {
        bail!(t().update_failed);
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn curl_file(url: &str, dest: &Path) -> Result<()> {
    let status = Command::new("curl")
        .args([
            "-fL",
            "--retry",
            "2",
            "--max-time",
            "120",
            "-H",
            "User-Agent: tuxius",
            "-H",
            "Cache-Control: no-cache",
            "-H",
            "Pragma: no-cache",
            "-o",
        ])
        .arg(dest)
        .arg(url)
        .status()
        .context(t().update_no_curl)?;
    if status.success() && dest.is_file() {
        Ok(())
    } else {
        bail!(t().update_failed)
    }
}

fn cache_path() -> Result<PathBuf> {
    Ok(store::config_dir()?.join("update.toml"))
}

fn load_cache() -> Option<Cache> {
    let text = fs::read_to_string(cache_path().ok()?).ok()?;
    toml::from_str(&text).ok()
}

fn save_cache(latest: &str) {
    let Some(path) = cache_path().ok() else {
        return;
    };
    let Some(dir) = path.parent() else { return };
    if fs::create_dir_all(dir).is_err() {
        return;
    }
    let cache = Cache {
        checked: now(),
        latest: latest.to_string(),
    };
    let Ok(text) = toml::to_string(&cache) else {
        return;
    };
    let Ok(mut file) = fs::File::create(path) else {
        return;
    };
    let _ = file.write_all(text.as_bytes());
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn json_string(json: &str, field: &str) -> Option<String> {
    let pattern = format!("\"{field}\"");
    let rest = &json[json.find(&pattern)? + pattern.len()..];
    let rest = rest.trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim().trim_start_matches('v');
    let mut parts = s.split('.');
    let version = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    if parts.next().is_some() {
        return None;
    }
    Some(version)
}

fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Result<Self> {
        let path = std::env::temp_dir().join(format!("tuxius-update-{}", std::process::id()));
        fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.3.2", "0.3.1"));
        assert!(is_newer("v0.4.0", "0.3.9"));
        assert!(!is_newer("0.3.1", "0.3.1"));
        assert!(!is_newer("0.3.0", "0.3.1"));
        assert_eq!(parse_version("1.2"), None);
    }

    #[test]
    fn reads_release_tag() {
        let body = r#"{"tag_name": "v0.3.2", "name": "v0.3.2"}"#;
        assert_eq!(json_string(body, "tag_name").as_deref(), Some("v0.3.2"));
    }
}
