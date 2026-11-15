use crate::i18n::{fill, t};
use crate::keys;
use crate::store::{Auth, Server};
use anyhow::{Context, Result, bail};
use std::process::{Command, Stdio};

/// When set, the binary acts as an `SSH_ASKPASS` helper and prints this value.
pub const ASKPASS_ENV: &str = "TUXIUS_ASKPASS_SECRET";

/// Called by ssh with the prompt text as the first argument.
pub fn askpass(secret: &str) -> i32 {
    let prompt = std::env::args().nth(1).unwrap_or_default().to_lowercase();
    if prompt.contains("password") {
        println!("{secret}");
        0
    } else {
        1
    }
}

fn stderr(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("Warning: Permanently added"))
        .collect();
    if lines.is_empty() {
        "connection failed".into()
    } else {
        lines.join(" ")
    }
}

/// `password` forces password login via the askpass helper; otherwise the
/// server's saved auth method is used.
fn command(server: &Server, password: Option<&str>, extra: &[&str]) -> Result<Command> {
    let mut cmd = Command::new("ssh");
    cmd.arg("-p").arg(server.port.to_string()).args([
        "-o",
        "StrictHostKeyChecking=accept-new",
        "-o",
        "ServerAliveInterval=30",
        "-o",
        "ConnectTimeout=10",
    ]);
    match (password, server.auth) {
        (Some(pw), _) => {
            cmd.args([
                "-o",
                "PubkeyAuthentication=no",
                "-o",
                "PreferredAuthentications=keyboard-interactive,password",
                "-o",
                "NumberOfPasswordPrompts=1",
            ]);
            cmd.env("SSH_ASKPASS", std::env::current_exe()?)
                .env("SSH_ASKPASS_REQUIRE", "force")
                .env(ASKPASS_ENV, pw);
            // OpenSSH < 8.4 ignores SSH_ASKPASS_REQUIRE and only uses askpass with DISPLAY set.
            if std::env::var_os("DISPLAY").is_none() {
                cmd.env("DISPLAY", ":0");
            }
        }
        (None, Auth::Key) => {
            cmd.arg("-i")
                .arg(keys::path(server.key_name())?)
                .args(["-o", "IdentitiesOnly=yes"]);
        }
        (None, Auth::Password) => {}
    }
    cmd.args(extra).arg("--").arg(server.target());
    Ok(cmd)
}

fn run(mut cmd: Command) -> Result<()> {
    let out = cmd
        .stdin(Stdio::null())
        .output()
        .context("could not run ssh (is OpenSSH installed?)")?;
    if out.status.success() {
        Ok(())
    } else {
        bail!("{}", stderr(&out.stderr))
    }
}

/// Uses the password once to append the public half of `key` to the server's
/// `authorized_keys`, then verifies that key login works.
pub fn install_key(server: &Server, password: &str, key: &str) -> Result<()> {
    if key == keys::DEFAULT {
        keys::ensure_default()?;
    }
    let pubkey = keys::public_key(key)?;
    let script = format!(
        "umask 077; mkdir -p ~/.ssh && touch ~/.ssh/authorized_keys && \
         (grep -qxF '{pubkey}' ~/.ssh/authorized_keys || echo '{pubkey}' >> ~/.ssh/authorized_keys)"
    );
    let mut cmd = command(server, Some(password), &[])?;
    cmd.arg(script);
    run(cmd)?;
    verify_key(server, key)
}

pub fn verify_key(server: &Server, key: &str) -> Result<()> {
    let mut probe = server.clone();
    probe.auth = Auth::Key;
    probe.key = Some(key.to_string());
    let mut cmd = command(&probe, None, &["-o", "BatchMode=yes"])?;
    cmd.arg("true");
    run(cmd).context(t().key_login_failed)
}

/// Removes the server's key from its `authorized_keys`, logging in with that key.
pub fn revoke_key(server: &Server) -> Result<()> {
    let pubkey = keys::public_key(server.key_name())?;
    let blob = pubkey
        .split_whitespace()
        .nth(1)
        .context("unexpected public key format")?;
    let script = format!(
        "f=~/.ssh/authorized_keys; [ -f \"$f\" ] || exit 0; \
         grep -vF '{blob}' \"$f\" > \"$f.tuxius\"; cat \"$f.tuxius\" > \"$f\"; rm -f \"$f.tuxius\""
    );
    let mut cmd = command(server, None, &["-o", "BatchMode=yes"])?;
    cmd.arg(script);
    run(cmd)
}

pub fn check_password(server: &Server, password: &str) -> Result<()> {
    let mut cmd = command(server, Some(password), &[])?;
    cmd.arg("true");
    run(cmd)
}

/// Hands the terminal to ssh: an interactive session, or `remote` run on the
/// server when it's not empty. Exits with ssh's status on Windows; replaces
/// the process elsewhere.
pub fn connect(server: &Server, remote: &[String]) -> Result<()> {
    if server.auth == Auth::Key && !keys::exists(server.key_name()) {
        bail!(fill(t().key_missing, &[server.key_name(), &server.name]));
    }
    let password = server.saved_password()?;
    let mut cmd = command(server, password.as_deref(), &[])?;
    cmd.args(remote);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(cmd.exec()).context("could not start ssh")
    }
    #[cfg(not(unix))]
    {
        let status = cmd
            .status()
            .context("could not run ssh (is OpenSSH installed?)")?;
        std::process::exit(status.code().unwrap_or(1));
    }
}
