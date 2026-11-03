#!/usr/bin/env bash
# Server updates from burrow admin through the real service manager (systemd or launchd), with the health check and rollback.
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
chmod 755 "$WORK"
PIDS=()
NAME=burrow-e2e-srv
OTHER=burrow-e2e-srvmate
case "$(uname)" in
  Darwin) STATE="/Library/Application Support/burrow" ;;
  *) STATE=/var/lib/burrow ;;
esac

cleanup() {
  set +e
  sudo "$WORK/s1" service uninstall "$OTHER" --purge >/dev/null 2>&1
  sudo "$WORK/s1" service uninstall "$NAME" --purge >/dev/null 2>&1
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null; done
  rm -rf "$WORK"
}
trap cleanup EXIT
fail() {
  echo "FAIL: $*" >&2
  sudo "$WORK/s1" service logs "$NAME" -n 40 >&2 || true
  sudo cat "$STATE/updates/e2e-srv/result.json" >&2 || true
  exit 1
}
check() { echo "  ok: $*"; }

mark() { cp "$OUT/burrow" "$WORK/$1"; printf '\n' >> "$WORK/$1"; local f="$1"; shift; for m in "$@"; do printf 'BURROW-TEST-%s\n' "$m" >> "$WORK/$f"; done; chmod 755 "$WORK/$f"; }
mark s1 VERSION=9.0.0-test.1
mark s2 VERSION=9.0.0-test.2
mark s3 VERSION=9.0.0-test.3 BREAK=config
mark s4 VERSION=9.0.0-test.4 BREAK=bind
mark s5 VERSION=9.0.0-test.5 BREAK=start
mark c1 VERSION=9.0.0-test.1

cd "$WORK"
mkfifo releases.ctl
"$SERVE" 9.0.0-test.2 s2 good 9.0.0-test.3 s3 good 9.0.0-test.4 s4 good 9.0.0-test.5 s5 good < releases.ctl > releases.url &
PIDS+=($!)
exec 3> releases.ctl
for _ in $(seq 1 600); do [ -s releases.url ] && break; sleep 0.2; done
URL="$(cat releases.url)"
[ -n "$URL" ] || fail "the release server did not start"
echo "latest 9.0.0-test.1" >&3

HOME_SECRET="$("$OUT/burrow" gensecret)"
OPS_SECRET="$("$OUT/burrow" gensecret)"
server_yaml() {
  cat <<YAML
listen: 127.0.0.1:27985
clients:
  home:
    secret: \${E2E_HOME}
visitors:
  ops:
    secret: \${E2E_OPS}
    admin: write
services:
  app: { client: home, bind: 127.0.0.1:27986, target: 127.0.0.1:27987 }
web:
  https: 127.0.0.1:27988
  request_headers:
    X-Releases: \${BURROW_UPDATE_URL}
    X-Window: \${BURROW_ADMIN_TEST_UPDATE_WINDOW}
    X-Latest: \${BURROW_ADMIN_TEST_LATEST_SECS}
$1
YAML
}
server_yaml "" > e2e-srv.yaml
cat > ops.yaml <<YAML
server: 127.0.0.1:27985
name: ops
secret: $OPS_SECRET
YAML
cat > home.yaml <<YAML
server: 127.0.0.1:27985
name: home
secret: $HOME_SECRET
allow: [127.0.0.1/32]
YAML
chmod 600 ops.yaml home.yaml
chmod 644 e2e-srv.yaml

install() {
  sudo E2E_HOME="$HOME_SECRET" E2E_OPS="$OPS_SECRET" BURROW_UPDATE_URL="$URL" BURROW_ADMIN_TEST_UPDATE_WINDOW=20 BURROW_ADMIN_TEST_LATEST_SECS=0 \
    "$@" 2>&1
}
own() { "$PROBE" ops.yaml status 2>/dev/null | python3 -c 'import json,sys; s=json.load(sys.stdin); print(json.dumps({"version": s.get("version"), "u": s.get("server_update"), "latest": (s.get("latest") or {}).get("version")}))'; }
field() { own | python3 -c "import json,sys; c=json.load(sys.stdin); v=c$1; print(v if v is not None else '')" 2>/dev/null || true; }
wait_for() {
  local what="$1" expr="$2" want="$3"
  for _ in $(seq 1 360); do [ "$(field "$expr")" = "$want" ] && return 0; sleep 0.5; done
  fail "$what: $(own)"
}
installed() { sudo "$(sudo cat "$(binary_of)" 2>/dev/null)" version 2>/dev/null; }
binary_of() { case "$(uname)" in Darwin) echo "/Library/Application Support/burrow/services/$NAME.json" ;; *) echo /var/lib/burrow/services/$NAME.json ;; esac; }
installed() { sudo "$(sudo cat "$(binary_of)" | python3 -c 'import json,sys; print(json.load(sys.stdin)["binary"])')" version; }
latest() {
  echo "latest $1" >&3
  wait_for "the server to see $1" "['latest']" "$1"
}
ask() { "$PROBE" ops.yaml call update_server '{}' 2>&1; }

echo "== a server installed without the opt-in refuses, with how to turn it on"
out="$(install ./s1 service install -c e2e-srv.yaml)" || fail "install: $out"
./c1 client -c home.yaml > home.log 2>&1 &
PIDS+=($!)
wait_for "the server reports itself" "['version']" 9.0.0-test.1
latest 9.0.0-test.2
[ "$(field "['u']['remote']")" = off ] || fail "remote updates are not off: $(own)"
field "['u']['why_not']" | grep -q "update.remote: true" || fail "no hint: $(own)"
r="$(ask)" && fail "an update was asked without the opt-in: $r"
echo "$r" | grep -q "update.remote: true" || fail "the refusal does not say how: $r"
check "no opt-in: refused with the hint"

echo "== update.remote: true written by the server account alone counts for nothing"
CONF="$(sudo cat "$(binary_of)" | python3 -c 'import json,sys; print(json.load(sys.stdin)["config"])')"
printf 'update:\n  remote: true\n' | sudo tee -a "$CONF" > /dev/null
sudo ./s1 service restart "$NAME" > /dev/null || fail "restart"
wait_for "the server back" "['version']" 9.0.0-test.1
field "['u']['why_not']" | grep -q "counts only once root installs" || fail "the file alone looks like an opt-in: $(own)"
r="$(ask)" && fail "an update was asked on the file alone: $r"
sudo ./s1 service remote-update "$NAME" > helper.out 2>&1 || true
sudo cat "$STATE/updates/e2e-srv/result.json" 2>/dev/null | grep -q "installed without remote updates" || fail "the root updater trusted server.yaml: $(cat helper.out)"
installed | grep -q 9.0.0-test.1 || fail "the binary changed: $(installed)"
check "server.yaml alone: refused by the server and by the root updater"

echo "== install with update.remote: true adds the updater"
server_yaml "update: { remote: true }" > e2e-srv.yaml
out="$(install ./s1 service install -c e2e-srv.yaml)" || fail "install: $out"
echo "$out" | grep -q "burrow mcp may update it" || fail "install does not mention the updater: $out"
wait_for "the server back" "['u']['remote']" on
check "remote updates on after a root install"

echo "== a binary shared with a service that did not opt in is never replaced"
mate_dir="$(mktemp -d)"; chmod 755 "$mate_dir"
cat > "$mate_dir/e2e-srvmate.yaml" <<YAML
server: 127.0.0.1:27985
name: home
secret: $HOME_SECRET
allow: [127.0.0.1/32]
YAML
chmod 644 "$mate_dir/e2e-srvmate.yaml"
out="$(sudo ./s1 service install -c "$mate_dir/e2e-srvmate.yaml" --no-start 2>&1)" || fail "install the mate: $out"
r="$(ask)" || fail "ask: $r"
wait_for "the refusal" "['u']['last']['state']" failed
field "['u']['last']['detail']" | grep -q "did not allow remote updates" || fail "no reason: $(own)"
installed | grep -q 9.0.0-test.1 || fail "the binary changed: $(installed)"
sudo ./s1 service uninstall "$OTHER" --purge > /dev/null || fail "uninstall the mate"
rm -rf "$mate_dir"
check "shared with a service without the opt-in: refused, nothing changed"

echo "== update to 9.0.0-test.2"
r="$(ask)" || fail "ask: $r"
echo "$r" | grep -q '"asked":true' || fail "not asked: $r"
r2="$(ask)" && fail "a second job was accepted: $r2"
wait_for "the update" "['u']['last']['state']" done
[ "$(field "['version']")" = 9.0.0-test.2 ] || fail "wrong version: $(own)"
field "['u']['last']['by']" | grep -q "^ops (" || fail "who asked is not recorded: $(own)"
field "['u']['last']['detail']" | grep -q "1 of 1 clients connected again" || fail "no reconnect count: $(own)"
installed | grep -q 9.0.0-test.2 || fail "the installed binary is $(installed)"
check "updated to 9.0.0-test.2, asked by ops, the client came back"

for case in "9.0.0-test.3:config refused" "9.0.0-test.4:a port that cannot bind" "9.0.0-test.5:a crash at start"; do
  v="${case%%:*}"; what="${case#*:}"
  echo "== $what is rolled back"
  latest "$v"
  r="$(ask)" || fail "ask $v: $r"
  wait_for "the rollback" "['u']['last']['state']" rolled-back
  wait_for "the old version back" "['version']" 9.0.0-test.2
  field "['u']['last']['detail']" | grep -q "restored 9.0.0-test.2" || fail "no reason: $(own)"
  installed | grep -q 9.0.0-test.2 || fail "the binary was not restored: $(installed)"
  check "$what: rolled back to 9.0.0-test.2 ($(field "['u']['last']['detail']" | cut -c1-90))"
done

echo "== uninstall removes the updater"
sudo ./s1 service uninstall "$NAME" --purge > /dev/null || fail "uninstall"
case "$(uname)" in
  Darwin) [ ! -e "/Library/LaunchDaemons/com.github.charlesng35.$NAME-update.plist" ] || fail "the updater job is left" ;;
  *) [ ! -e "/etc/systemd/system/$NAME-update.path" ] || fail "the updater path unit is left"
     left="$(systemctl list-units --all --no-legend "$NAME*" 2>/dev/null)"; [ -z "$left" ] || fail "units are left: $left" ;;
esac
[ ! -e "$STATE/updates/e2e-srv" ] || fail "the update folder is left"
check "updater removed"
echo "server-update e2e: all ok"
