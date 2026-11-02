#!/usr/bin/env bash
# Remote client updates through the real service manager (systemd or launchd), with rollback.
# Needs sudo. Builds with the admin-test-hooks feature, so the binaries can pretend to be other versions.
set -euo pipefail

# BURROW_E2E_BIN=DIR uses burrow, admin_probe and release_server from DIR instead of building them.
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
if [ -n "${BURROW_E2E_BIN:-}" ]; then
  OUT="$BURROW_E2E_BIN"
  PROBE="$OUT/admin_probe"
  SERVE="$OUT/release_server"
else
  cargo build --manifest-path "$ROOT/Cargo.toml" --features admin-test-hooks --bin burrow --example admin_probe --example release_server -q
  OUT="${CARGO_TARGET_DIR:-$ROOT/target}/debug"
  PROBE="$OUT/examples/admin_probe"
  SERVE="$OUT/examples/release_server"
fi
WORK="$(mktemp -d)"
PIDS=()
NAME=burrow-e2e-upd

cleanup() {
  set +e
  sudo "$WORK/v1" service uninstall "$NAME" --purge >/dev/null 2>&1
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null; done
  rm -rf "$WORK"
}
trap cleanup EXIT
fail() {
  echo "FAIL: $*" >&2
  sudo "$WORK/v1" service logs "$NAME" -n 40 >&2 || true
  exit 1
}
check() { echo "  ok: $*"; }

mark() { cp "$OUT/burrow" "$WORK/$1"; printf '\n' >> "$WORK/$1"; shift_name="$1"; shift; for m in "$@"; do printf 'BURROW-TEST-%s\n' "$m" >> "$WORK/$shift_name"; done; chmod 755 "$WORK/$shift_name"; }
mark v1 VERSION=9.0.0-test.1
mark v2 VERSION=9.0.0-test.2
mark v3 VERSION=9.0.0-test.3
mark v4 VERSION=9.0.0-test.4 BREAK=connect
mark v5 VERSION=9.0.0-test.5 BREAK=start
mark v6 VERSION=9.0.0-test.6 BREAK=config

cd "$WORK"
mkfifo releases.ctl
"$SERVE" 9.0.0-test.2 v2 good 9.0.0-test.3 v3 tampered 9.0.0-test.4 v4 good 9.0.0-test.5 v5 good 9.0.0-test.6 v6 good < releases.ctl > releases.url &
PIDS+=($!)
exec 3> releases.ctl
for _ in $(seq 1 600); do [ -s releases.url ] && break; sleep 0.2; done
URL="$(cat releases.url)"
[ -n "$URL" ] || fail "the release server did not start"

HOME_SECRET="$("$OUT/burrow" gensecret)"
OPS_SECRET="$("$OUT/burrow" gensecret)"
cat > server.yaml <<YAML
listen: 127.0.0.1:27975
clients:
  home:
    secret: $HOME_SECRET
visitors:
  ops:
    secret: $OPS_SECRET
    admin: write
YAML
cat > ops.yaml <<YAML
server: 127.0.0.1:27975
name: ops
secret: $OPS_SECRET
YAML
cat > e2e-upd.yaml <<YAML
server: 127.0.0.1:27975
name: home
secret: \${E2E_SECRET}
allow: [127.0.0.1/32]
headers:
  X-Releases: \${BURROW_UPDATE_URL}
  X-Window: \${BURROW_ADMIN_TEST_UPDATE_WINDOW}
update:
  remote: true
YAML
chmod 600 server.yaml ops.yaml e2e-upd.yaml
BURROW_UPDATE_URL="$URL" BURROW_ADMIN_TEST_LATEST_SECS=0 "$OUT/burrow" server -c server.yaml > server.log 2>&1 &
PIDS+=($!)

home() { "$PROBE" ops.yaml status 2>/dev/null | python3 -c 'import json,sys; s=json.load(sys.stdin); c=[c for c in s["clients"] if c["name"]=="home"][0]; print(json.dumps(c))'; }
field() { home | python3 -c "import json,sys; c=json.load(sys.stdin); v=c$1; print(v if v is not None else '')" 2>/dev/null || true; }
wait_for() {
  local what="$1" expr="$2" want="$3"
  for _ in $(seq 1 240); do [ "$(field "$expr")" = "$want" ] && return 0; sleep 0.5; done
  fail "$what: $(home)"
}
ask() {
  echo "latest $1" >&3
  wait_for "the server to see $1" "['available']" "$1"
  "$PROBE" ops.yaml call update_clients '{"clients":["home"]}' || fail "ask $1"
}
installed() { sudo "$(sudo cat /var/lib/burrow/services/$NAME.json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["binary"])' 2>/dev/null || echo /usr/local/bin/burrow)" version; }

echo "== install the client as a service that allows remote updates"
out="$(sudo E2E_SECRET="$HOME_SECRET" BURROW_UPDATE_URL="$URL" BURROW_ADMIN_TEST_UPDATE_WINDOW=8 ./v1 service install -c e2e-upd.yaml 2>&1)" || fail "install: $out"
echo "$out" | grep -q "the server's admin may ask it to update" || fail "install does not mention the updater: $out"
wait_for "the old version reports" "['build']['version']" 9.0.0-test.1
[ "$(field "['build']['remote']")" = on ] || fail "remote updates are not on: $(home)"
[ "$(field "['build']['run']")" = service ] || fail "not seen as a service: $(home)"
sudo ./v1 status "$NAME" | grep -q "remote updates  allowed" || fail "burrow status on the client: $(sudo ./v1 status "$NAME")"
check "the client reports 9.0.0-test.1 as a service, remote updates allowed"

echo "== update to 9.0.0-test.2"
ask 9.0.0-test.2
wait_for "the update" "['update']['state']" done
[ "$(field "['build']['version']")" = 9.0.0-test.2 ] || fail "wrong version after the update: $(home)"
installed | grep -q 9.0.0-test.2 || fail "the installed binary is $(installed)"
check "updated to 9.0.0-test.2 through the service manager"

echo "== a download that does not match its checksum changes nothing"
ask 9.0.0-test.3
wait_for "the failure" "['update']['state']" failed
field "['update']['detail']" | grep -q SHA-256 || fail "no checksum reason: $(home)"
installed | grep -q 9.0.0-test.2 || fail "the binary changed: $(installed)"
check "checksum mismatch: nothing changed"

echo "== a version that does not connect is rolled back"
ask 9.0.0-test.4
wait_for "the rollback" "['update']['state']" rolled-back
field "['update']['detail']" | grep -q "did not connect" || fail "no reason: $(home)"
wait_for "the old version back" "['build']['version']" 9.0.0-test.2
installed | grep -q 9.0.0-test.2 || fail "the binary was not restored: $(installed)"
check "no connection: rolled back to 9.0.0-test.2"

echo "== a version that keeps crashing is rolled back"
ask 9.0.0-test.5
wait_for "the rollback" "['update']['state']" rolled-back
wait_for "the old version back" "['build']['version']" 9.0.0-test.2
installed | grep -q 9.0.0-test.2 || fail "the binary was not restored: $(installed)"
check "crash loop: rolled back to 9.0.0-test.2"

echo "== a version that does not accept the config changes nothing"
ask 9.0.0-test.6
wait_for "the failure" "['update']['state']" failed
field "['update']['detail']" | grep -q "does not accept" || fail "no reason: $(home)"
installed | grep -q 9.0.0-test.2 || fail "the binary changed: $(installed)"
check "config refused by the new version: nothing changed"

echo "== uninstall removes the updater"
sudo ./v1 service uninstall "$NAME" --purge >/dev/null || fail "uninstall"
case "$(uname)" in
  Darwin) [ ! -e "/Library/LaunchDaemons/com.github.charlesng35.$NAME-update.plist" ] || fail "the updater job is left" ;;
  *) [ ! -e "/etc/systemd/system/$NAME-update.path" ] || fail "the updater path unit is left" ;;
esac
case "$(uname)" in
  Darwin) [ ! -e "/Library/Application Support/burrow/updates/e2e-upd" ] || fail "the update folder is left" ;;
  *) [ ! -e /var/lib/burrow/e2e-upd ] && [ ! -e /var/lib/burrow/updates/e2e-upd ] || fail "the update folders are left" ;;
esac
check "updater removed"
echo "remote-update e2e: all ok"
