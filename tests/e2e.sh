#!/usr/bin/env bash
# End-to-end test against a throwaway OpenSSH server in Docker.
set -euo pipefail

cd "$(dirname "$0")/.."
PORT=${TUXIUS_TEST_PORT:-2222}
PASSWORD="tuxius-test-pw"
IMAGE=tuxius-test-sshd
CONTAINER=tuxius-test-sshd

pass() { printf '\033[32m✔\033[0m %s\n' "$1"; }
fail() { printf '\033[31m✘\033[0m %s\n' "$1"; exit 1; }

cargo build --quiet
BIN="$(cargo metadata --format-version 1 --no-deps | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')/debug/tuxius"
[ -x "$BIN" ] || fail "binary not found at $BIN"

docker build --quiet -t "$IMAGE" - >/dev/null <<EOF
FROM alpine:3.20
RUN apk add --no-cache openssh && ssh-keygen -A \
 && echo "root:$PASSWORD" | chpasswd \
 && sed -i 's/^#\?PermitRootLogin.*/PermitRootLogin yes/; s/^#\?PasswordAuthentication.*/PasswordAuthentication yes/' /etc/ssh/sshd_config
CMD ["/usr/sbin/sshd", "-D", "-e"]
EOF

SANDBOX=$(mktemp -d)
cleanup() {
    docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
    ssh-keygen -R "[127.0.0.1]:$PORT" >/dev/null 2>&1 || true
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
docker run -d --name "$CONTAINER" -p "127.0.0.1:$PORT:22" "$IMAGE" >/dev/null
for _ in $(seq 30); do
    ssh-keyscan -p "$PORT" 127.0.0.1 >/dev/null 2>&1 && break
    sleep 1
done

export TUXIUS_HOME="$SANDBOX/config" NO_COLOR=1
mkdir -p "$SANDBOX/home"
tx() { HOME="$SANDBOX/home" "$BIN" "$@"; }

echo "$PASSWORD" | tx add --name box --host "root@127.0.0.1:$PORT" --password-stdin
[ -f "$TUXIUS_HOME/keys/tuxius" ] || fail "key was not generated"
grep -q 'auth = "key"' "$TUXIUS_HOME/servers.toml" || fail "server not saved with key auth"
grep -q password "$TUXIUS_HOME/servers.toml" && fail "password must not be stored for key auth"
pass "add with key auth"

[ "$(tx box -- echo hello)" = hello ] || fail "key login"
pass "connect with key"

[ "$(tx connect bo -- whoami)" = root ] || fail "prefix match"
pass "connect by prefix"

echo "$PASSWORD" | tx add --name box-pw --host 127.0.0.1 --port "$PORT" --auth password --password-stdin
grep -q 'auth = "password"' "$TUXIUS_HOME/servers.toml" || fail "password auth not saved"
[ "$(tx box-pw -- echo via-password)" = via-password ] || fail "password login"
pass "add + connect with saved password"

[ "$(stat -c %a "$TUXIUS_HOME/servers.toml" 2>/dev/null || stat -f %Lp "$TUXIUS_HOME/servers.toml")" = 600 ] \
    || fail "servers.toml must be chmod 600"
pass "config file is private (600)"

if echo "wrong" | tx add --name bad --host "root@127.0.0.1:$PORT" --password-stdin 2>/dev/null; then
    fail "wrong password should not be saved"
fi
grep -q 'name = "bad"' "$TUXIUS_HOME/servers.toml" && fail "bad server was saved"
pass "wrong password is rejected"

if echo "$PASSWORD" | tx add --name box --host 127.0.0.1 --password-stdin 2>/dev/null; then
    fail "duplicate name should be rejected"
fi
pass "duplicate names are rejected"

tx list | grep -q box-pw || fail "list"
pass "list"

tx rm box-pw --yes
tx list | grep -q box-pw && fail "remove"
pass "remove"

ssh-keygen -q -t ed25519 -N "" -C mine -f "$SANDBOX/mykey"
tx key add --name mine --file "$SANDBOX/mykey"
[ "$(stat -c %a "$TUXIUS_HOME/keys/mine" 2>/dev/null || stat -f %Lp "$TUXIUS_HOME/keys/mine")" = 600 ] \
    || fail "imported key must be chmod 600"
tx key list | grep -q mine || fail "key list"
pass "import key"

echo "$PASSWORD" | tx add --name own --host "root@127.0.0.1:$PORT" --key mine --password-stdin
grep -q 'key = "mine"' "$TUXIUS_HOME/servers.toml" || fail "server should use the imported key"
[ "$(tx own -- echo own-key)" = own-key ] || fail "login with imported key"
pass "add server with own key + connect"

if tx key rm mine --yes 2>/dev/null; then
    fail "a key in use must not be deletable"
fi
pass "key in use is protected"

tx rm own --yes --revoke
if ssh -p "$PORT" -i "$TUXIUS_HOME/keys/mine" -o IdentitiesOnly=yes -o BatchMode=yes \
    -o StrictHostKeyChecking=no root@127.0.0.1 true 2>/dev/null; then
    fail "revoked key still works"
fi
[ "$(tx box -- echo still-ok)" = still-ok ] || fail "revoking one key must not affect others"
pass "remove server + revoke its key on the server"

tx key rm mine --yes
tx key list | grep -q mine && fail "key remove"
pass "delete key"

tx key add --generate --name fresh | grep -q '^ssh-ed25519 ' || fail "generate should print the public key"
[ "$(tx key show fresh)" = "$(cat "$TUXIUS_HOME/keys/fresh.pub")" ] || fail "key show"
pass "generate + show key"

echo
pass "all e2e tests passed"
