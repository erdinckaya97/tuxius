#!/bin/sh
# Tuxius installer — https://github.com/erdinckaya97/tuxius
#
#   curl -fsSL https://raw.githubusercontent.com/erdinckaya97/tuxius/main/install.sh | sh
#
# Environment:
#   TUXIUS_VERSION      release tag to install (default: latest)
#   TUXIUS_INSTALL_DIR  where to put the binary (default: /usr/local/bin or ~/.local/bin)
set -eu

REPO="erdinckaya97/tuxius"
BIN="tuxius"

say() { printf '  \033[38;5;45m›\033[0m %s\n' "$1"; }
ok() { printf '  \033[38;5;84m✔\033[0m %s\n' "$1"; }
die() { printf '  \033[38;5;203m✘\033[0m %s\n' "$1" >&2; exit 1; }

need() { command -v "$1" >/dev/null 2>&1; }

download() {
    if need curl; then
        curl -fsSL "$1" -o "$2"
    elif need wget; then
        wget -qO "$2" "$1"
    else
        die "curl or wget is required"
    fi
}

detect_target() {
    os=$(uname -s)
    arch=$(uname -m)
    case "$arch" in
        x86_64 | amd64) arch=x86_64 ;;
        arm64 | aarch64) arch=aarch64 ;;
        *) die "unsupported CPU architecture: $arch" ;;
    esac
    case "$os" in
        Linux) echo "${arch}-unknown-linux-musl" ;;
        Darwin) echo "${arch}-apple-darwin" ;;
        *) die "unsupported OS: $os (Tuxius runs on Linux and macOS)" ;;
    esac
}

pick_dir() {
    if [ -n "${TUXIUS_INSTALL_DIR:-}" ]; then
        echo "$TUXIUS_INSTALL_DIR"
    elif [ -w /usr/local/bin ] || { [ "$(id -u)" = 0 ] && mkdir -p /usr/local/bin; }; then
        echo /usr/local/bin
    elif need sudo && [ -d /usr/local/bin ] && [ -t 0 -o -e /dev/tty ]; then
        echo /usr/local/bin
    else
        echo "$HOME/.local/bin"
    fi
}

install_bin() {
    src=$1
    dir=$2
    if mkdir -p "$dir" 2>/dev/null && [ -w "$dir" ]; then
        install -m 755 "$src" "$dir/$BIN"
    else
        say "Installing to $dir requires sudo"
        sudo mkdir -p "$dir"
        sudo install -m 755 "$src" "$dir/$BIN"
    fi
}

main() {
    need ssh || die "OpenSSH client (ssh) is required"
    target=$(detect_target)
    version=${TUXIUS_VERSION:-latest}
    if [ "$version" = latest ]; then
        url="https://github.com/$REPO/releases/latest/download/$BIN-$target.tar.gz"
    else
        url="https://github.com/$REPO/releases/download/$version/$BIN-$target.tar.gz"
    fi

    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT

    say "Downloading Tuxius ($target)"
    if download "$url" "$tmp/$BIN.tar.gz" 2>/dev/null; then
        tar -xzf "$tmp/$BIN.tar.gz" -C "$tmp"
    elif need cargo; then
        say "No prebuilt binary found, building from source with cargo"
        cargo install --git "https://github.com/$REPO" --root "$tmp/cargo" --quiet
        cp "$tmp/cargo/bin/$BIN" "$tmp/$BIN"
    else
        die "could not download $url"
    fi

    dir=$(pick_dir)
    install_bin "$tmp/$BIN" "$dir"
    ok "Installed $("$dir/$BIN" --version) to $dir/$BIN"

    case ":$PATH:" in
        *":$dir:"*) ;;
        *)
            rc="$HOME/.profile"
            case "${SHELL:-}" in
                */zsh) rc="$HOME/.zshrc" ;;
                */bash) rc="$HOME/.bashrc" ;;
            esac
            printf '\nexport PATH="%s:$PATH"\n' "$dir" >>"$rc"
            ok "Added $dir to PATH in $rc — open a new terminal or run: export PATH=\"$dir:\$PATH\""
            ;;
    esac

    printf '\n  Type \033[1mtuxius\033[0m to add your first server.\n\n'
}

main "$@"
