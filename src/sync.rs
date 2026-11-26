//! An encrypted copy of the servers and keys, kept in a folder the user
//! already syncs or on a WebDAV cloud (Nextcloud, ownCloud, a server they host).
//!
//! On a Mac with iCloud Drive, that folder is offered first. The passphrase
//! stays on this computer; the file in the cloud is ciphertext.

use crate::app;
use crate::i18n::{fill, t};
use crate::keys;
use crate::private;
use crate::secrets;
use crate::store::{Auth, Server, Store, config_dir, now_secs};
use crate::ui;
use crate::vault::{self, Bundle, DecryptFailed, Stamp, VaultKey, VaultServer};
use anyhow::{Context, Result, bail};
use inquire::validator::Validation;
use inquire::{Password, PasswordDisplayMode, Select, Text};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const VAULT_NAME: &str = "tuxius.vault";
const PASSPHRASE_ACCOUNT: &str = "sync-passphrase";
const WEBDAV_ACCOUNT: &str = "sync-webdav";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Provider {
    Icloud,
    Folder,
    Webdav,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Settings {
    provider: Provider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    user: Option<String>,
    /// Only set when this computer has no keychain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    passphrase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    webdav_password: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Journal {
    #[serde(default, rename = "key")]
    keys: Vec<Stamp>,
    #[serde(default, rename = "removed_server")]
    removed_servers: Vec<Stamp>,
    #[serde(default, rename = "removed_key")]
    removed_keys: Vec<Stamp>,
}

enum Choice {
    Icloud(PathBuf),
    Folder,
    Address,
}

impl std::fmt::Display for Choice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Choice::Icloud(_) => t().sync_icloud,
            Choice::Folder => t().sync_folder,
            Choice::Address => t().sync_address,
        };
        f.write_str(label)
    }
}

enum Next {
    Now,
    Change,
}

impl std::fmt::Display for Next {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Next::Now => t().sync_now,
            Next::Change => t().sync_change,
        })
    }
}

pub fn place() -> Option<String> {
    let settings = load_settings().ok().flatten()?;
    Some(describe(&settings))
}

/// Sync now, or ask where the copy should live when nothing is set up.
pub fn run(store: &mut Store) -> Result<()> {
    if load_settings()?.is_none() {
        if !std::io::stdin().is_terminal() {
            bail!(t().sync_off);
        }
        return setup(store);
    }
    ui::step(t().sync_working);
    reconcile(store)?;
    ui::ok(t().sync_done);
    Ok(())
}

pub fn setup(store: &mut Store) -> Result<()> {
    let previous = load_settings()?;
    if previous.is_some() && !app::confirm(t().sync_replace_q, false)? {
        return Ok(());
    }
    configure(store, previous)
}

pub fn interactive(store: &mut Store) -> Result<()> {
    if load_settings()?.is_none() {
        return setup(store);
    }
    match Select::new(t().menu_sync, vec![Next::Now, Next::Change]).prompt()? {
        Next::Now => {
            ui::step(t().sync_working);
            reconcile(store)?;
            ui::ok(t().sync_done);
            Ok(())
        }
        Next::Change => configure(store, load_settings()?),
    }
}

/// Pull on startup. Quiet when nothing changed; a warning when the cloud is unreachable.
pub fn refresh(store: &mut Store) {
    if load_settings().ok().flatten().is_none() {
        return;
    }
    match reconcile(store) {
        Ok(true) => ui::ok(t().sync_done),
        Ok(false) => {}
        Err(err) => ui::warn(&fill(t().sync_failed, &[&format!("{err:#}")])),
    }
}

pub fn publish(store: &mut Store) {
    if load_settings().ok().flatten().is_none() {
        return;
    }
    if let Err(err) = reconcile(store) {
        ui::warn(&fill(t().sync_failed, &[&format!("{err:#}")]));
    }
}

pub fn publish_saved() {
    match Store::load() {
        Ok(mut store) => publish(&mut store),
        Err(err) => ui::warn(&fill(t().sync_failed, &[&format!("{err:#}")])),
    }
}

pub fn note_key(name: &str) -> Result<()> {
    let mut journal = load_journal()?;
    upsert(&mut journal.keys, name, now_secs());
    journal.removed_keys.retain(|stamp| stamp.name != name);
    save_journal(&journal)
}

pub fn forget_key(name: &str) -> Result<()> {
    let mut journal = load_journal()?;
    journal.keys.retain(|stamp| stamp.name != name);
    upsert(&mut journal.removed_keys, name, now_secs());
    save_journal(&journal)
}

pub fn forget_server(name: &str) -> Result<()> {
    let mut journal = load_journal()?;
    upsert(&mut journal.removed_servers, name, now_secs());
    save_journal(&journal)
}

fn upsert(stamps: &mut Vec<Stamp>, name: &str, changed: u64) {
    if let Some(stamp) = stamps.iter_mut().find(|stamp| stamp.name == name) {
        stamp.changed = changed;
    } else {
        stamps.push(Stamp {
            name: name.to_string(),
            changed,
        });
    }
}

fn configure(store: &mut Store, previous: Option<Settings>) -> Result<()> {
    let first = previous.is_none();
    let old_phrase = previous
        .as_ref()
        .and_then(|settings| passphrase_of(settings).ok());
    let old_webdav = previous
        .as_ref()
        .and_then(|settings| webdav_password(settings).ok());

    let choice = destination()?;
    let mut settings = match choice {
        Choice::Icloud(root) => {
            let dir = root.join("Tuxius");
            private::create_dir(&dir)
                .with_context(|| fill(t().sync_bad_folder, &[&dir.display().to_string()]))?;
            Settings {
                provider: Provider::Icloud,
                path: Some(dir.display().to_string()),
                url: None,
                user: None,
                passphrase: None,
                webdav_password: None,
            }
        }
        Choice::Folder => Settings {
            provider: Provider::Folder,
            path: Some(prompt_folder()?),
            url: None,
            user: None,
            passphrase: None,
            webdav_password: None,
        },
        Choice::Address => {
            let (url, user, password) = prompt_address()?;
            Settings {
                provider: Provider::Webdav,
                path: None,
                url: Some(url),
                user: Some(user),
                passphrase: None,
                webdav_password: remember(WEBDAV_ACCOUNT, &password)?,
            }
        }
    };
    let phrase = match old_phrase.clone() {
        Some(phrase) => phrase,
        None => prompt_phrase()?,
    };
    settings.passphrase = remember(PASSPHRASE_ACCOUNT, &phrase)?;
    save_settings(&settings)?;
    if first {
        baseline(store)?;
    }

    match reconcile(store) {
        Ok(_) => {
            ui::ok(&fill(t().sync_ready, &[&describe(&settings)]));
            Ok(())
        }
        Err(err) => {
            if let Some(prev) = previous {
                save_settings(&prev)?;
                if let Some(phrase) = old_phrase {
                    let _ = secrets::set(PASSPHRASE_ACCOUNT, &phrase);
                }
                if let Some(password) = old_webdav {
                    let _ = secrets::set(WEBDAV_ACCOUNT, &password);
                }
            } else {
                clear_settings()?;
            }
            Err(err)
        }
    }
}

fn destination() -> Result<Choice> {
    let mut choices = Vec::new();
    if let Some(dir) = icloud_dir() {
        choices.push(Choice::Icloud(dir));
    }
    choices.push(Choice::Folder);
    choices.push(Choice::Address);
    Ok(Select::new(t().sync_where, choices).prompt()?)
}

fn icloud_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = dirs::home_dir()?;
        let dir = home.join("Library/Mobile Documents/com~apple~CloudDocs");
        dir.is_dir().then_some(dir)
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

fn prompt_folder() -> Result<String> {
    ui::step(t().sync_vault_note);
    let raw = Text::new(t().sync_folder_q)
        .with_placeholder(t().sync_folder_ph)
        .with_help_message(t().sync_folder_help)
        .with_validator(|input: &str| {
            Ok(if input.trim().is_empty() {
                Validation::Invalid(fill(t().sync_bad_folder, &[" "]).into())
            } else {
                Validation::Valid
            })
        })
        .prompt()?;
    let path = keys::expand_home(raw.trim());
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    };
    if path.is_file() {
        bail!(fill(t().sync_bad_folder, &[&path.display().to_string()]));
    }
    private::create_dir(&path)
        .with_context(|| fill(t().sync_bad_folder, &[&path.display().to_string()]))?;
    Ok(path.display().to_string())
}

fn prompt_address() -> Result<(String, String, String)> {
    ui::step(t().sync_vault_note);
    let url = Text::new(t().sync_url_q)
        .with_placeholder(t().sync_url_ph)
        .with_help_message(t().sync_url_help)
        .with_validator(|input: &str| {
            Ok(match vault_url(input.trim()) {
                Ok(_) => Validation::Valid,
                Err(err) => Validation::Invalid(err.to_string().into()),
            })
        })
        .prompt()?;
    let url = vault_url(url.trim())?;
    // Store the directory; `vault_url` appends the file name when downloading.
    let url = url
        .strip_suffix(&format!("/{VAULT_NAME}"))
        .unwrap_or(&url)
        .to_string();
    let user = Text::new(t().sync_user_q)
        .with_validator(|input: &str| {
            Ok(if input.trim().is_empty() {
                Validation::Invalid(t().name_empty.into())
            } else {
                Validation::Valid
            })
        })
        .prompt()?;
    let password = Password::new(t().sync_cloud_pw)
        .without_confirmation()
        .with_display_mode(PasswordDisplayMode::Masked)
        .with_display_toggle_enabled()
        .with_help_message(t().sync_cloud_pw_help)
        .prompt()?;
    Ok((url, user.trim().to_string(), password))
}

fn prompt_phrase() -> Result<String> {
    Ok(Password::new(t().sync_phrase_q)
        .with_display_mode(PasswordDisplayMode::Masked)
        .with_display_toggle_enabled()
        .with_help_message(t().sync_phrase_help)
        .with_validator(|input: &str| {
            Ok(if input.chars().count() >= 8 {
                Validation::Valid
            } else {
                Validation::Invalid(t().sync_phrase_short.into())
            })
        })
        .with_custom_confirmation_message(t().sync_phrase_q)
        .with_custom_confirmation_error_message(t().sync_phrase_mismatch)
        .prompt()?)
}

/// The first setup stamps everything on this computer as "now", so an empty
/// second computer receives this copy instead of the other way around.
fn baseline(store: &mut Store) -> Result<()> {
    let now = now_secs();
    for server in &mut store.servers {
        server.changed = Some(now);
    }
    store.save()?;
    let mut journal = load_journal()?;
    journal.keys.clear();
    for name in key_names()? {
        journal.keys.push(Stamp { name, changed: now });
    }
    for stamp in &mut journal.removed_servers {
        stamp.changed = now;
    }
    for stamp in &mut journal.removed_keys {
        stamp.changed = now;
    }
    save_journal(&journal)
}

/// `true` when this computer's server list or keys changed.
fn reconcile(store: &mut Store) -> Result<bool> {
    let settings = load_settings()?.context(t().sync_off)?;
    let phrase = passphrase_of(&settings)?;
    let local = export(store)?;
    let remote = match download(&settings)? {
        None => Bundle::default(),
        Some(text) => vault::open(&text, &phrase)?,
    };
    let merged = vault::merge(local.clone(), remote.clone());
    let changed = merged.servers != local.servers || merged.keys != local.keys;
    if merged != local {
        apply(store, &merged)?;
    }
    if merged != remote {
        upload(&settings, &vault::seal(&merged, &phrase)?)?;
    }
    Ok(changed)
}

fn export(store: &Store) -> Result<Bundle> {
    let journal = load_journal()?;
    let mut servers = Vec::new();
    for server in &store.servers {
        let password = if server.auth == Auth::Password {
            server.saved_password()?
        } else {
            None
        };
        servers.push(VaultServer {
            name: server.name.clone(),
            host: server.host.clone(),
            user: server.user.clone(),
            port: server.port,
            auth: server.auth,
            key: server.key.clone(),
            password,
            last_used: server.last_used,
            changed: server.changed.unwrap_or(0),
        });
    }
    let mut keys = Vec::new();
    let dir = keys::dir()?;
    if dir.exists() {
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            let Some(name) = key_file_name(&path) else {
                continue;
            };
            let private_key = fs::read_to_string(&path)
                .with_context(|| format!("could not read {}", path.display()))?;
            let public_key = fs::read_to_string(keys::pub_of(&path)).ok();
            let changed = journal
                .keys
                .iter()
                .find(|stamp| stamp.name == name)
                .map(|stamp| stamp.changed)
                .unwrap_or_else(|| modified(&path));
            keys.push(VaultKey {
                name: name.to_string(),
                private_key,
                public_key,
                changed,
            });
        }
    }
    let mut bundle = Bundle {
        servers,
        keys,
        removed_servers: journal.removed_servers,
        removed_keys: journal.removed_keys,
    };
    sort_bundle(&mut bundle);
    Ok(bundle)
}

fn apply(store: &mut Store, bundle: &Bundle) -> Result<()> {
    for server in &store.servers {
        let kept = bundle
            .servers
            .iter()
            .any(|item| item.name.eq_ignore_ascii_case(&server.name));
        if server.keychain && !kept {
            let _ = secrets::delete(&server.name);
        }
    }
    store.servers = bundle
        .servers
        .iter()
        .map(|item| Server {
            name: item.name.clone(),
            host: item.host.clone(),
            user: item.user.clone(),
            port: item.port,
            auth: item.auth,
            key: item.key.clone(),
            password: item.password.clone(),
            keychain: false,
            last_used: item.last_used,
            changed: Some(item.changed),
        })
        .collect();
    store.save()?;

    let dir = keys::dir()?;
    private::create_dir(&dir)?;
    let mut keep = Vec::new();
    for key in &bundle.keys {
        if !safe_key_name(&key.name) {
            continue;
        }
        let path = dir.join(&key.name);
        write_secret(&path, key.private_key.as_bytes())?;
        if let Some(public) = &key.public_key {
            let mut text = public.trim().to_string();
            text.push('\n');
            fs::write(keys::pub_of(&path), text)?;
        }
        keep.push(key.name.clone());
    }
    if dir.exists() {
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            let Some(name) = key_file_name(&path) else {
                continue;
            };
            if !keep.iter().any(|kept| kept == name) {
                let _ = fs::remove_file(&path);
                let _ = fs::remove_file(keys::pub_of(&path));
            }
        }
    }
    save_journal(&Journal {
        keys: bundle
            .keys
            .iter()
            .map(|key| Stamp {
                name: key.name.clone(),
                changed: key.changed,
            })
            .collect(),
        removed_servers: bundle.removed_servers.clone(),
        removed_keys: bundle.removed_keys.clone(),
    })
}

fn sort_bundle(bundle: &mut Bundle) {
    bundle
        .servers
        .sort_by_key(|server| server.name.to_lowercase());
    bundle.keys.sort_by_key(|key| key.name.to_lowercase());
    bundle
        .removed_servers
        .sort_by_key(|stamp| stamp.name.to_lowercase());
    bundle
        .removed_keys
        .sort_by_key(|stamp| stamp.name.to_lowercase());
}

fn download(settings: &Settings) -> Result<Option<String>> {
    match settings.provider {
        Provider::Icloud | Provider::Folder => {
            let path = PathBuf::from(settings.path.clone().unwrap_or_default());
            let file = path.join(VAULT_NAME);
            if !file.exists() {
                return Ok(None);
            }
            Ok(Some(fs::read_to_string(&file).with_context(|| {
                fill(t().sync_bad_folder, &[&file.display().to_string()])
            })?))
        }
        Provider::Webdav => {
            let url = vault_url(settings.url.as_deref().unwrap_or(""))?;
            let user = settings.user.clone().unwrap_or_default();
            let password = webdav_password(settings)?;
            let response = curl(&url, &user, &password, None)?;
            match response.code {
                200 => String::from_utf8(response.body)
                    .map(Some)
                    .map_err(|_| DecryptFailed.into()),
                404 => Ok(None),
                401 | 403 => bail!(t().sync_login),
                code => bail!(fill(t().sync_failed, &[&code.to_string()])),
            }
        }
    }
}

fn upload(settings: &Settings, text: &str) -> Result<()> {
    match settings.provider {
        Provider::Icloud | Provider::Folder => {
            let path = PathBuf::from(settings.path.clone().unwrap_or_default());
            write_vault(&path, text)
        }
        Provider::Webdav => {
            let url = vault_url(settings.url.as_deref().unwrap_or(""))?;
            let user = settings.user.clone().unwrap_or_default();
            let password = webdav_password(settings)?;
            let tmp = std::env::temp_dir().join(format!(
                "tuxius-vault-{}-{}",
                std::process::id(),
                now_secs()
            ));
            write_secret(&tmp, text.as_bytes())?;
            let response = curl(&url, &user, &password, Some(&tmp));
            let _ = fs::remove_file(&tmp);
            let response = response?;
            let response = if matches!(response.code, 404 | 409) {
                if let Some(parent) = url.rsplit_once('/').map(|(parent, _)| parent) {
                    let _ = curl_mkcol(parent, &user, &password);
                }
                let tmp = std::env::temp_dir().join(format!(
                    "tuxius-vault-{}-{}-b",
                    std::process::id(),
                    now_secs()
                ));
                write_secret(&tmp, text.as_bytes())?;
                let retry = curl(&url, &user, &password, Some(&tmp));
                let _ = fs::remove_file(&tmp);
                retry?
            } else {
                response
            };
            match response.code {
                200 | 201 | 204 => Ok(()),
                401 | 403 => bail!(t().sync_login),
                code => bail!(fill(t().sync_failed, &[&code.to_string()])),
            }
        }
    }
}

fn write_vault(dir: &Path, text: &str) -> Result<()> {
    private::create_dir(dir)
        .with_context(|| fill(t().sync_bad_folder, &[&dir.display().to_string()]))?;
    let tmp = dir.join(format!("{VAULT_NAME}.tmp"));
    write_secret(&tmp, text.as_bytes())?;
    fs::rename(&tmp, dir.join(VAULT_NAME))
        .with_context(|| fill(t().sync_bad_folder, &[&dir.display().to_string()]))?;
    Ok(())
}

struct Http {
    code: u16,
    body: Vec<u8>,
}

fn curl(url: &str, user: &str, password: &str, upload: Option<&Path>) -> Result<Http> {
    let config = curl_config(user, password)?;
    let body = std::env::temp_dir().join(format!("tuxius-http-{}", std::process::id()));
    let mut cmd = Command::new("curl");
    cmd.args([
        "-sS",
        "--max-time",
        "60",
        "--retry",
        "2",
        "-L",
        "-A",
        "tuxius",
    ])
    .arg("--config")
    .arg(&config.0)
    .arg("-o")
    .arg(&body)
    .args(["-w", "%{http_code}"]);
    if let Some(file) = upload {
        cmd.arg("-T").arg(file);
    }
    cmd.arg(url).stdin(Stdio::null()).stderr(Stdio::piped());
    let out = cmd
        .output()
        .map_err(|_| anyhow::anyhow!(t().sync_no_curl))?;
    let code = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0);
    let bytes = fs::read(&body).unwrap_or_default();
    let _ = fs::remove_file(&body);
    drop(config);
    if !out.status.success() && code == 0 {
        let detail = String::from_utf8_lossy(&out.stderr);
        let detail = detail.trim();
        bail!(if detail.is_empty() {
            t().sync_no_curl.to_string()
        } else {
            detail.to_string()
        });
    }
    Ok(Http { code, body: bytes })
}

fn curl_mkcol(url: &str, user: &str, password: &str) -> Result<()> {
    let config = curl_config(user, password)?;
    let status = Command::new("curl")
        .args(["-sS", "--max-time", "30", "-X", "MKCOL", "-A", "tuxius"])
        .arg("--config")
        .arg(&config.0)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| anyhow::anyhow!(t().sync_no_curl))?;
    drop(config);
    if status.success() {
        Ok(())
    } else {
        bail!(t().sync_failed)
    }
}

struct CurlConfig(PathBuf);

impl Drop for CurlConfig {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn curl_config(user: &str, password: &str) -> Result<CurlConfig> {
    if user.contains(['\n', '\r', '"']) || password.contains(['\n', '\r']) {
        bail!(t().sync_login);
    }
    let path =
        std::env::temp_dir().join(format!("tuxius-curl-{}-{}", std::process::id(), now_secs()));
    write_secret(&path, curl_user_line(user, password).as_bytes())?;
    Ok(CurlConfig(path))
}

fn curl_user_line(user: &str, password: &str) -> String {
    let raw = format!("{user}:{password}");
    let escaped = raw.replace('\\', "\\\\").replace('"', "\\\"");
    format!("user = \"{escaped}\"\n")
}

fn vault_url(input: &str) -> Result<String> {
    let input = input.trim().trim_end_matches('/');
    if input.chars().any(char::is_whitespace)
        || !(input.starts_with("https://") || input.starts_with("http://"))
    {
        bail!(t().sync_bad_url);
    }
    if input.rsplit('/').next() == Some(VAULT_NAME) {
        Ok(input.to_string())
    } else {
        Ok(format!("{input}/{VAULT_NAME}"))
    }
}

fn describe(settings: &Settings) -> String {
    match settings.provider {
        Provider::Icloud => settings.path.clone().unwrap_or_else(|| "iCloud".into()),
        Provider::Folder => settings.path.clone().unwrap_or_default(),
        Provider::Webdav => settings.url.clone().unwrap_or_default(),
    }
}

fn remember(account: &str, secret: &str) -> Result<Option<String>> {
    if secrets::available() && secrets::set(account, secret).is_ok() {
        Ok(None)
    } else {
        Ok(Some(secret.to_string()))
    }
}

fn passphrase_of(settings: &Settings) -> Result<String> {
    if let Some(phrase) = &settings.passphrase {
        return Ok(phrase.clone());
    }
    secrets::get(PASSPHRASE_ACCOUNT).map_err(|_| anyhow::anyhow!(t().sync_decrypt))
}

fn webdav_password(settings: &Settings) -> Result<String> {
    if let Some(password) = &settings.webdav_password {
        return Ok(password.clone());
    }
    secrets::get(WEBDAV_ACCOUNT).map_err(|_| anyhow::anyhow!(t().sync_login))
}

fn settings_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("sync.toml"))
}

fn journal_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("journal.toml"))
}

fn load_settings() -> Result<Option<Settings>> {
    let path = settings_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let text =
        fs::read_to_string(&path).with_context(|| format!("could not read {}", path.display()))?;
    Ok(Some(toml::from_str(&text).with_context(|| {
        format!("{} is not valid", path.display())
    })?))
}

fn save_settings(settings: &Settings) -> Result<()> {
    let path = settings_path()?;
    if let Some(dir) = path.parent() {
        private::create_dir(dir)?;
    }
    let text = toml::to_string_pretty(settings)?;
    let mut file = private::create_file(&path, false)?;
    file.write_all(text.as_bytes())?;
    Ok(())
}

fn clear_settings() -> Result<()> {
    let path = settings_path()?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    let _ = secrets::delete(PASSPHRASE_ACCOUNT);
    let _ = secrets::delete(WEBDAV_ACCOUNT);
    Ok(())
}

fn load_journal() -> Result<Journal> {
    let path = journal_path()?;
    if !path.exists() {
        return Ok(Journal::default());
    }
    let text = fs::read_to_string(&path)?;
    Ok(toml::from_str(&text).unwrap_or_default())
}

fn save_journal(journal: &Journal) -> Result<()> {
    let path = journal_path()?;
    if let Some(dir) = path.parent() {
        private::create_dir(dir)?;
    }
    let text = toml::to_string_pretty(journal)?;
    let mut file = private::create_file(&path, false)?;
    file.write_all(text.as_bytes())?;
    Ok(())
}

fn key_names() -> Result<Vec<String>> {
    let dir = keys::dir()?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for entry in fs::read_dir(dir)? {
        if let Some(name) = key_file_name(&entry?.path()) {
            names.push(name.to_string());
        }
    }
    Ok(names)
}

fn key_file_name(path: &Path) -> Option<&str> {
    let name = path.file_name()?.to_str()?;
    if path.is_file() && safe_key_name(name) {
        Some(name)
    } else {
        None
    }
}

fn safe_key_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.ends_with(".pub")
        && !name.contains(['/', '\\'])
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

fn modified(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|time| time.as_secs())
        .unwrap_or(0)
}

fn write_secret(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = private::create_file(path, false)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_vault_url() {
        assert_eq!(
            vault_url("https://cloud.example/remote.php/dav/files/ada").unwrap(),
            "https://cloud.example/remote.php/dav/files/ada/tuxius.vault"
        );
        assert_eq!(
            vault_url("https://cloud.example/tuxius.vault").unwrap(),
            "https://cloud.example/tuxius.vault"
        );
        assert!(vault_url("cloud.example/files").is_err());
        assert!(vault_url("https://cloud.example/has space").is_err());
    }

    #[test]
    fn quotes_the_curl_password() {
        assert_eq!(
            curl_user_line("ada", r#"a"b\c"#),
            "user = \"ada:a\\\"b\\\\c\"\n"
        );
    }

    #[test]
    fn writes_only_ciphertext_into_the_folder() {
        let dir = std::env::temp_dir().join(format!("tuxius-sync-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let bundle = Bundle {
            servers: vec![VaultServer {
                name: "web".into(),
                host: "10.9.8.7".into(),
                user: "root".into(),
                port: 22,
                auth: Auth::Password,
                key: None,
                password: Some("not-in-the-file".into()),
                last_used: None,
                changed: 3,
            }],
            ..Bundle::default()
        };
        let sealed = vault::seal(&bundle, "a long passphrase").unwrap();
        write_vault(&dir, &sealed).unwrap();
        let text = fs::read_to_string(dir.join(VAULT_NAME)).unwrap();
        assert!(!text.contains("not-in-the-file"));
        assert!(!text.contains("10.9.8.7"));
        assert_eq!(vault::open(&text, "a long passphrase").unwrap(), bundle);
        let _ = fs::remove_dir_all(&dir);
    }
}
