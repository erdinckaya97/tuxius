use crate::i18n::{fill, t};
use crate::private;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Auth {
    Key,
    Password,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Server {
    pub name: String,
    pub host: String,
    #[serde(default = "default_user")]
    pub user: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub auth: Auth,
    /// Key name in the Tuxius key store; `None` means the default key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<u64>,
}

fn default_user() -> String {
    "root".into()
}

fn default_port() -> u16 {
    22
}

impl Server {
    pub fn target(&self) -> String {
        format!("{}@{}", self.user, self.host)
    }

    pub fn address(&self) -> String {
        if self.port == 22 {
            self.target()
        } else {
            format!("{}:{}", self.target(), self.port)
        }
    }

    pub fn key_name(&self) -> &str {
        self.key.as_deref().unwrap_or(crate::keys::DEFAULT)
    }

    pub fn auth_label(&self) -> String {
        match (self.auth, self.key.as_deref()) {
            (Auth::Password, _) => t().auth_password.into(),
            (Auth::Key, None) => t().auth_key.into(),
            (Auth::Key, Some(k)) => format!("{}: {k}", t().auth_key),
        }
    }

    pub fn touch(&mut self) {
        self.last_used = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs());
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Store {
    /// Interface language chosen with `tuxius lang`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    #[serde(default, rename = "server")]
    pub servers: Vec<Server>,
    #[serde(skip)]
    path: PathBuf,
}

pub fn config_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("TUXIUS_HOME") {
        return Ok(PathBuf::from(dir));
    }
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir).join("tuxius"));
    }
    let home = dirs::home_dir().context("could not find your home directory")?;
    Ok(home.join(".config").join("tuxius"))
}

impl Store {
    pub fn load() -> Result<Self> {
        let path = config_dir()?.join("servers.toml");
        let mut store = if path.exists() {
            let text = fs::read_to_string(&path)
                .with_context(|| format!("could not read {}", path.display()))?;
            toml::from_str::<Store>(&text)
                .with_context(|| format!("{} is not valid", path.display()))?
        } else {
            Store::default()
        };
        store.path = path;
        Ok(store)
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn save(&self) -> Result<()> {
        let dir = self.path.parent().context("invalid config path")?;
        private::create_dir(dir)?;

        let tmp = self.path.with_extension("toml.tmp");
        let text = toml::to_string_pretty(self)?;
        {
            let mut file = private::create_file(&tmp, false)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
        }
        fs::rename(&tmp, &self.path)
            .with_context(|| format!("could not write {}", self.path.display()))?;
        Ok(())
    }

    /// Exact (case-insensitive) match first, then a unique prefix match.
    pub fn find(&self, query: &str) -> Result<usize> {
        let q = query.to_lowercase();
        if let Some(i) = self.servers.iter().position(|s| s.name.to_lowercase() == q) {
            return Ok(i);
        }
        let hits: Vec<usize> = self
            .servers
            .iter()
            .enumerate()
            .filter(|(_, s)| s.name.to_lowercase().starts_with(&q))
            .map(|(i, _)| i)
            .collect();
        match hits.as_slice() {
            [i] => Ok(*i),
            [] => bail!(fill(t().no_server_named, &[query])),
            _ => {
                let names: Vec<&str> = hits
                    .iter()
                    .map(|&i| self.servers[i].name.as_str())
                    .collect();
                bail!(fill(t().ambiguous, &[query, &names.join(", ")]))
            }
        }
    }

    pub fn servers_using_key(&self, key: &str) -> Vec<&str> {
        self.servers
            .iter()
            .filter(|s| s.auth == Auth::Key && s.key_name() == key)
            .map(|s| s.name.as_str())
            .collect()
    }

    pub fn name_taken(&self, name: &str, except: Option<usize>) -> bool {
        self.servers
            .iter()
            .enumerate()
            .any(|(i, s)| Some(i) != except && s.name.eq_ignore_ascii_case(name))
    }

    /// Indices ordered by most recently used, then by name.
    pub fn ordered(&self) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..self.servers.len()).collect();
        idx.sort_by(|&a, &b| {
            let (a, b) = (&self.servers[a], &self.servers[b]);
            b.last_used
                .cmp(&a.last_used)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        idx
    }
}

/// Accepts `host`, `user@host`, `host:port` and `user@host:port`.
pub fn parse_destination(input: &str) -> (Option<String>, String, Option<u16>) {
    let input = input.trim();
    let (user, rest) = match input.rsplit_once('@') {
        Some((u, r)) if !u.is_empty() => (Some(u.to_string()), r),
        _ => (None, input),
    };
    // Leave bare IPv6 addresses alone; `[v6]:port` is still supported.
    if let Some((host, tail)) = rest.strip_prefix('[').and_then(|r| r.split_once(']')) {
        let port = tail.strip_prefix(':').and_then(|p| p.parse().ok());
        return (user, host.to_string(), port);
    }
    if rest.matches(':').count() == 1
        && let Some((host, port)) = rest.split_once(':')
        && let Ok(port) = port.parse()
    {
        return (user, host.to_string(), Some(port));
    }
    (user, rest.to_string(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(name: &str, last_used: Option<u64>) -> Server {
        Server {
            name: name.into(),
            host: "10.0.0.1".into(),
            user: "root".into(),
            port: 22,
            auth: Auth::Key,
            key: None,
            password: None,
            last_used,
        }
    }

    #[test]
    fn parses_destinations() {
        assert_eq!(parse_destination("1.2.3.4"), (None, "1.2.3.4".into(), None));
        assert_eq!(
            parse_destination("admin@example.com"),
            (Some("admin".into()), "example.com".into(), None)
        );
        assert_eq!(
            parse_destination("root@1.2.3.4:2222"),
            (Some("root".into()), "1.2.3.4".into(), Some(2222))
        );
        assert_eq!(parse_destination("::1"), (None, "::1".into(), None));
        assert_eq!(
            parse_destination("[::1]:22"),
            (None, "::1".into(), Some(22))
        );
    }

    #[test]
    fn finds_by_name_and_prefix() {
        let store = Store {
            lang: None,
            servers: vec![
                server("web-prod", None),
                server("web-dev", None),
                server("db", None),
            ],
            path: PathBuf::new(),
        };
        assert_eq!(store.find("DB").unwrap(), 2);
        assert_eq!(store.find("web-p").unwrap(), 0);
        assert!(store.find("web").is_err());
        assert!(store.find("nope").is_err());
    }

    #[test]
    fn orders_by_recent_use() {
        let store = Store {
            lang: None,
            servers: vec![
                server("b", None),
                server("a", Some(5)),
                server("c", Some(9)),
            ],
            path: PathBuf::new(),
        };
        assert_eq!(store.ordered(), vec![2, 1, 0]);
    }

    #[test]
    fn roundtrips_toml() {
        let mut s = server("web", Some(1));
        s.auth = Auth::Password;
        s.password = Some("secret".into());
        let store = Store {
            lang: None,
            servers: vec![s.clone()],
            path: PathBuf::new(),
        };
        let text = toml::to_string_pretty(&store).unwrap();
        assert!(text.contains("[[server]]"));
        let back: Store = toml::from_str(&text).unwrap();
        assert_eq!(back.servers, vec![s]);
    }
}
