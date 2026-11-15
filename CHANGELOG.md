# Changelog

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
