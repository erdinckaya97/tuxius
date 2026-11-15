//! Files and folders only the current user can open.

use std::fs::{DirBuilder, File, OpenOptions};
use std::io;
use std::path::Path;

pub fn create_dir(dir: &Path) -> io::Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}

/// Opens `path` for writing; with `create_new` it fails if the file exists.
pub fn create_file(path: &Path, create_new: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true).truncate(true);
    }
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let file = options.open(path)?;
    #[cfg(windows)]
    restrict(path);
    Ok(file)
}

/// OpenSSH for Windows refuses private keys that other accounts can read.
#[cfg(windows)]
fn restrict(path: &Path) {
    let Ok(user) = std::env::var("USERNAME") else {
        return;
    };
    let account = match std::env::var("USERDOMAIN") {
        Ok(domain) if !domain.is_empty() => format!("{domain}\\{user}"),
        _ => user,
    };
    let _ = std::process::Command::new("icacls")
        .arg(path)
        .args(["/inheritance:r", "/grant:r", &format!("{account}:F")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}
