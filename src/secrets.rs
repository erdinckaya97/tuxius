//! Saved passwords live in the system keychain when there is one: Keychain
//! on macOS, Credential Manager on Windows, Secret Service (GNOME Keyring,
//! KWallet) on Linux.

/// Keychain entries are grouped per config folder, so a custom
/// `TUXIUS_HOME` doesn't share passwords with the default one.
fn service() -> String {
    match std::env::var("TUXIUS_HOME") {
        Ok(home) if !home.is_empty() => format!("tuxius:{home}"),
        _ => "tuxius".into(),
    }
}

pub use imp::{available, delete, get, set};

#[cfg(any(target_os = "macos", windows))]
mod imp {
    use super::service;
    use anyhow::Result;
    use keyring::{Entry, Error};

    pub fn available() -> bool {
        true
    }

    pub fn set(name: &str, password: &str) -> Result<()> {
        Entry::new(&service(), name)?.set_password(password)?;
        Ok(())
    }

    pub fn get(name: &str) -> Result<String> {
        Ok(Entry::new(&service(), name)?.get_password()?)
    }

    pub fn delete(name: &str) -> Result<()> {
        match Entry::new(&service(), name)?.delete_credential() {
            Ok(()) | Err(Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    use super::service;
    use anyhow::{Context, Result, bail};
    use std::io::Write;
    use std::process::{Command, Stdio};

    /// Servers and containers usually have no keyring; the config file is
    /// used there instead.
    pub fn available() -> bool {
        std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some()
            && std::env::var_os("PATH").is_some_and(|path| {
                std::env::split_paths(&path).any(|dir| dir.join("secret-tool").is_file())
            })
    }

    fn secret_tool(args: &[&str], name: &str) -> Command {
        let mut cmd = Command::new("secret-tool");
        cmd.args(args)
            .args(["service", &service(), "account", name])
            .stderr(Stdio::piped());
        cmd
    }

    pub fn set(name: &str, password: &str) -> Result<()> {
        let label = format!("Tuxius: {name}");
        let mut child = secret_tool(&["store", "--label", &label], name)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .context("could not run secret-tool")?;
        child
            .stdin
            .take()
            .context("could not talk to secret-tool")?
            .write_all(password.as_bytes())?;
        let out = child.wait_with_output()?;
        if !out.status.success() {
            bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(())
    }

    pub fn get(name: &str) -> Result<String> {
        let out = secret_tool(&["lookup"], name)
            .stdin(Stdio::null())
            .output()
            .context("could not run secret-tool")?;
        if !out.status.success() {
            bail!("no saved password in the keyring");
        }
        let mut secret = String::from_utf8(out.stdout)?;
        if secret.ends_with('\n') {
            secret.pop();
        }
        Ok(secret)
    }

    pub fn delete(name: &str) -> Result<()> {
        secret_tool(&["clear"], name)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .status()
            .context("could not run secret-tool")?;
        Ok(())
    }
}

/// Exercises the real keychain; run with `TUXIUS_KEYCHAIN_TEST=1 cargo test`.
#[cfg(test)]
mod tests {
    #[test]
    fn round_trip() -> anyhow::Result<()> {
        if std::env::var_os("TUXIUS_KEYCHAIN_TEST").is_none() {
            return Ok(());
        }
        assert!(super::available());
        let name = format!("test-{}", std::process::id());
        super::set(&name, "pässwörd 1")?;
        assert_eq!(super::get(&name)?, "pässwörd 1");
        super::set(&name, "second")?;
        assert_eq!(super::get(&name)?, "second");
        super::delete(&name)?;
        assert!(super::get(&name).is_err());
        super::delete(&name)?;
        Ok(())
    }
}
