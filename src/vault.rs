//! The encrypted file that sync stores in a cloud folder.
//!
//! The passphrase never leaves this computer. What gets written is
//! `tuxius-vault 1`, the Argon2id parameters, and the ciphertext.

use crate::i18n::t;
use crate::store::Auth;
use anyhow::Result;
use argon2::{Algorithm, Argon2, Params, Version};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chacha20poly1305::aead::Aead;
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub struct DecryptFailed;

impl std::fmt::Display for DecryptFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(t().sync_decrypt)
    }
}

impl std::error::Error for DecryptFailed {}

#[derive(Clone, Copy)]
struct Kdf {
    m: u32,
    t: u32,
    p: u32,
}

/// OWASP's Argon2id recommendation: 19 MiB, two passes.
const STRONG: Kdf = Kdf {
    m: 19 * 1024,
    t: 2,
    p: 1,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    pub name: String,
    pub changed: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VaultServer {
    pub name: String,
    pub host: String,
    #[serde(default = "root")]
    pub user: String,
    #[serde(default = "port_22")]
    pub port: u16,
    pub auth: Auth,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<u64>,
    #[serde(default)]
    pub changed: u64,
}

fn root() -> String {
    "root".into()
}

fn port_22() -> u16 {
    22
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VaultKey {
    pub name: String,
    #[serde(rename = "private")]
    pub private_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
    #[serde(default)]
    pub changed: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Bundle {
    #[serde(default, rename = "server")]
    pub servers: Vec<VaultServer>,
    #[serde(default, rename = "key")]
    pub keys: Vec<VaultKey>,
    #[serde(default, rename = "removed_server")]
    pub removed_servers: Vec<Stamp>,
    #[serde(default, rename = "removed_key")]
    pub removed_keys: Vec<Stamp>,
}

pub fn seal(bundle: &Bundle, passphrase: &str) -> Result<String> {
    encrypt(toml::to_string(bundle)?.as_bytes(), passphrase, STRONG)
}

pub fn open(text: &str, passphrase: &str) -> Result<Bundle> {
    let bytes = decrypt(text, passphrase)?;
    toml::from_slice(&bytes).map_err(|_| DecryptFailed.into())
}

/// Newer copy wins. A deletion wins when it is newer than the copy.
/// Equal timestamps keep the local copy, and the later `last_used`.
pub fn merge(local: Bundle, remote: Bundle) -> Bundle {
    let (servers, removed_servers) = merge_group(
        local.servers,
        remote.servers,
        &local.removed_servers,
        &remote.removed_servers,
        |s| s.name.clone(),
        |s| s.changed,
        |mut win: VaultServer, other: VaultServer| {
            win.last_used = win.last_used.max(other.last_used);
            win
        },
    );
    let (keys, removed_keys) = merge_group(
        local.keys,
        remote.keys,
        &local.removed_keys,
        &remote.removed_keys,
        |k| k.name.clone(),
        |k| k.changed,
        |win, _| win,
    );
    Bundle {
        servers,
        keys,
        removed_servers,
        removed_keys,
    }
}

fn merge_group<T: Clone>(
    local: Vec<T>,
    remote: Vec<T>,
    local_removed: &[Stamp],
    remote_removed: &[Stamp],
    name_of: impl Fn(&T) -> String,
    changed_of: impl Fn(&T) -> u64,
    finish: impl Fn(T, T) -> T,
) -> (Vec<T>, Vec<Stamp>) {
    let mut keys = Vec::new();
    let mut add = |name: &str| {
        let key = name.to_lowercase();
        if !keys.iter().any(|have: &String| have == &key) {
            keys.push(key);
        }
    };
    for item in local.iter().chain(&remote) {
        add(&name_of(item));
    }
    for stamp in local_removed.iter().chain(remote_removed) {
        add(&stamp.name);
    }

    let mut kept = Vec::new();
    let mut removed = Vec::new();
    for key in keys {
        let local_item = local
            .iter()
            .find(|item| name_of(item).to_lowercase() == key);
        let remote_item = remote
            .iter()
            .find(|item| name_of(item).to_lowercase() == key);
        let winner = match (local_item, remote_item) {
            (Some(left), Some(right)) => {
                let (win, lose) = if changed_of(right) > changed_of(left) {
                    (right, left)
                } else {
                    (left, right)
                };
                Some(finish(win.clone(), lose.clone()))
            }
            (Some(left), None) => Some(left.clone()),
            (None, Some(right)) => Some(right.clone()),
            (None, None) => None,
        };
        let tomb = local_removed
            .iter()
            .chain(remote_removed)
            .filter(|stamp| stamp.name.to_lowercase() == key)
            .max_by_key(|stamp| stamp.changed);
        let item_changed = winner.as_ref().map(&changed_of).unwrap_or(0);
        let tomb_changed = tomb.map(|stamp| stamp.changed).unwrap_or(0);
        if item_changed >= tomb_changed
            && let Some(item) = winner
        {
            kept.push(item);
        } else if let Some(stamp) = tomb.filter(|_| tomb_changed > 0) {
            removed.push(stamp.clone());
        }
    }
    kept.sort_by_key(|item| name_of(item).to_lowercase());
    removed.sort_by_key(|stamp| stamp.name.to_lowercase());
    (kept, removed)
}

fn encrypt(plain: &[u8], passphrase: &str, kdf: Kdf) -> Result<String> {
    let salt = random::<16>();
    let nonce = random::<24>();
    let key = derive(passphrase, &salt, kdf)?;
    let cipher = XChaCha20Poly1305::new_from_slice(&key).expect("32-byte key");
    let ciphertext = cipher
        .encrypt(XNonce::from_slice(&nonce), plain)
        .expect("encryption");
    Ok(format!(
        "tuxius-vault 1\nm={} t={} p={}\n{}\n{}\n{}\n",
        kdf.m,
        kdf.t,
        kdf.p,
        STANDARD.encode(salt),
        STANDARD.encode(nonce),
        STANDARD.encode(ciphertext),
    ))
}

fn decrypt(text: &str, passphrase: &str) -> Result<Vec<u8>> {
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    if lines.next() != Some("tuxius-vault 1") {
        return Err(DecryptFailed.into());
    }
    let Some(kdf) = lines.next().and_then(parse_kdf) else {
        return Err(DecryptFailed.into());
    };
    if kdf.m > 256 * 1024 || kdf.t == 0 || kdf.t > 10 || kdf.p == 0 || kdf.p > 8 {
        return Err(DecryptFailed.into());
    }
    let Some(salt) = lines
        .next()
        .and_then(|line| STANDARD.decode(line.trim()).ok())
    else {
        return Err(DecryptFailed.into());
    };
    let Some(nonce) = lines
        .next()
        .and_then(|line| STANDARD.decode(line.trim()).ok())
    else {
        return Err(DecryptFailed.into());
    };
    let Some(ciphertext) = lines
        .next()
        .and_then(|line| STANDARD.decode(line.trim()).ok())
    else {
        return Err(DecryptFailed.into());
    };
    if nonce.len() != 24 {
        return Err(DecryptFailed.into());
    }
    let key = derive(passphrase, &salt, kdf).map_err(|_| DecryptFailed)?;
    let cipher = XChaCha20Poly1305::new_from_slice(&key).map_err(|_| DecryptFailed)?;
    cipher
        .decrypt(XNonce::from_slice(&nonce), ciphertext.as_ref())
        .map_err(|_| DecryptFailed.into())
}

fn parse_kdf(line: &str) -> Option<Kdf> {
    let mut m = None;
    let mut t = None;
    let mut p = None;
    for part in line.split_whitespace() {
        let (name, value) = part.split_once('=')?;
        let value: u32 = value.parse().ok()?;
        match name {
            "m" => m = Some(value),
            "t" => t = Some(value),
            "p" => p = Some(value),
            _ => return None,
        }
    }
    Some(Kdf {
        m: m?,
        t: t?,
        p: p?,
    })
}

fn derive(passphrase: &str, salt: &[u8], kdf: Kdf) -> Result<[u8; 32]> {
    let params = Params::new(kdf.m, kdf.t, kdf.p, Some(32)).map_err(|_| DecryptFailed)?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; 32];
    argon
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|_| DecryptFailed)?;
    Ok(key)
}

fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes
}

#[cfg(test)]
fn seal_fast(bundle: &Bundle, passphrase: &str) -> Result<String> {
    encrypt(
        toml::to_string(bundle)?.as_bytes(),
        passphrase,
        Kdf { m: 8, t: 1, p: 1 },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(name: &str, host: &str, changed: u64) -> VaultServer {
        VaultServer {
            name: name.into(),
            host: host.into(),
            user: "root".into(),
            port: 22,
            auth: Auth::Key,
            key: None,
            password: None,
            last_used: None,
            changed,
        }
    }

    #[test]
    fn round_trips_and_hides_the_secret() {
        let bundle = Bundle {
            servers: vec![VaultServer {
                password: Some("s3cret-password".into()),
                auth: Auth::Password,
                ..server("web", "10.0.0.1", 4)
            }],
            ..Bundle::default()
        };
        let file = seal_fast(&bundle, "correct horse battery").unwrap();
        assert!(!file.contains("s3cret-password"));
        assert!(!file.contains("10.0.0.1"));
        assert_eq!(open(&file, "correct horse battery").unwrap(), bundle);
        assert!(open(&file, "wrong passphrase!!").is_err());
    }

    #[test]
    fn newer_copy_wins_and_a_newer_delete_removes_it() {
        let local = Bundle {
            servers: vec![server("web", "old", 1), server("only-local", "1.1.1.1", 3)],
            removed_servers: vec![Stamp {
                name: "gone".into(),
                changed: 9,
            }],
            ..Bundle::default()
        };
        let remote = Bundle {
            servers: vec![
                server("web", "new", 5),
                server("gone", "9.9.9.9", 2),
                server("only-remote", "2.2.2.2", 4),
            ],
            ..Bundle::default()
        };
        let merged = merge(local, remote);
        let hosts: Vec<&str> = merged.servers.iter().map(|s| s.host.as_str()).collect();
        assert_eq!(hosts, vec!["1.1.1.1", "2.2.2.2", "new"]);
        assert_eq!(merged.removed_servers.len(), 1);
        assert_eq!(merged.removed_servers[0].name, "gone");
    }

    #[test]
    fn an_edit_after_a_delete_comes_back() {
        let local = Bundle {
            servers: vec![server("web", "revived", 10)],
            ..Bundle::default()
        };
        let remote = Bundle {
            removed_servers: vec![Stamp {
                name: "web".into(),
                changed: 4,
            }],
            ..Bundle::default()
        };
        let merged = merge(local, remote);
        assert_eq!(merged.servers.len(), 1);
        assert!(merged.removed_servers.is_empty());
    }
}
