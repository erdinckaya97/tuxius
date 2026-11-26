# Changelog

## v0.4.0

- `tuxius sync` keeps an encrypted copy of your servers, saved passwords, and SSH keys. On a Mac with iCloud Drive, iCloud is offered. Every system can also use a synced folder (Dropbox, OneDrive, Google Drive, Syncthing, or any other) or a WebDAV address (Nextcloud, ownCloud, or a cloud you host)
- The copy is encrypted on this computer before it is written. The passphrase stays here
- The installer asks GitHub which release is current, then downloads that exact file. The old "latest" link kept serving the previous version for a while after a release

## v0.3.2

- `tuxius update` downloads and installs the latest release
- When a newer release is out, Tuxius says so and points at `tuxius update`

## v0.3.1

- The port question no longer snaps back to 22. A spare Enter from the previous question was accepted as "keep 22" before a different port could be typed, and the suggestion stayed on screen beside whatever you wrote

## v0.3.0

- Saved passwords are kept in the system keychain (macOS Keychain, Windows Credential Manager, GNOME Keyring / KWallet) instead of the config file; existing ones move there automatically
- Windows: the interface follows the Windows display language
- Users and hosts starting with `-` are rejected, and ssh never reads them as options

## v0.2.0

- Windows support (10 / 11, x64 and ARM): works in PowerShell, cmd and Windows Terminal with the built-in OpenSSH client
- Windows installer: `irm https://raw.githubusercontent.com/erdinckaya97/tuxius/main/install.ps1 | iex`

## v0.1.0

First release.

- Interactive server picker: type `tuxius`, pick a server, you're connected
- Add, edit and remove servers; fuzzy search; most recently used first
- Log in with a new Tuxius key (password used once), your own SSH key, or a saved password
- SSH key manager: import, generate, show and delete keys (`tuxius key`)
- Removing a server can also remove its key from the server (`tuxius rm --revoke`)
- Direct connect (`tuxius web`) and remote commands (`tuxius web -- df -h`)
- Interface in English, Türkçe and Русский — follows the system language, change with `tuxius lang`
- Prebuilt binaries for Linux (x86_64, arm64) and macOS (Intel, Apple Silicon)
