#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PROFILE="${BURROW_E2E_PROFILE:-release}"
cargo build --profile "$PROFILE" --manifest-path "$ROOT/Cargo.toml" --bin burrow --example loadgen --example admin_probe -q
OUT="$ROOT/target/$([ "$PROFILE" = dev ] && echo debug || echo "$PROFILE")"
B="$OUT/burrow"
LOADGEN="$OUT/examples/loadgen"
PROBE="$OUT/examples/admin_probe"
case "$(uname)" in
  Darwin) STATE="/Library/Application Support/burrow/services" ;;
  *) STATE=/var/lib/burrow ;;
esac
WORK="$(mktemp -d)"
PIDS=()

cleanup() {
  set +e
  sudo "$B" service uninstall burrow-e2e-visitor --purge >/dev/null 2>&1
  sudo "$B" service uninstall burrow-e2e-admin --purge >/dev/null 2>&1
  sudo "$B" service uninstall burrow-e2e-client --purge >/dev/null 2>&1
  sudo "$B" service uninstall burrow-e2e-server --purge >/dev/null 2>&1
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null; done
  [ -n "${SOCK_PID:-}" ] && sudo kill "$SOCK_PID" 2>/dev/null
  sudo rm -rf "${SOCK_DIR:-/nonexistent-burrow}"
  rm -rf "$WORK"
}
trap cleanup EXIT
[ -n "${GITHUB_ACTIONS:-}" ] && trap 'echo "::error title=service e2e::line $LINENO failed: $BASH_COMMAND"' ERR

fail() {
  echo "FAIL: $*" >&2
  if [ -n "${GITHUB_ACTIONS:-}" ]; then
    echo "::error title=service e2e::$(printf '%s' "$*" | tr '\n' ' ')"
    for n in burrow-e2e-server burrow-e2e-client burrow-e2e-visitor; do
      echo "::error title=$n logs::$(sudo "$B" service logs "$n" -n 15 2>&1 | tr '\n' '|')"
    done
  fi
  for n in burrow-e2e-server burrow-e2e-client burrow-e2e-visitor; do sudo "$B" service logs "$n" -n 30 >&2 || true; done; exit 1; }
check() { echo "  ok: $*"; }
running() {
  for _ in $(seq 1 50); do
    sudo "$B" service status "$1" 2>/dev/null | grep -q "^$1: running" && return 0
    sleep 0.2
  done
  return 1
}

cd "$WORK"
"$LOADGEN" echo --listen 127.0.0.1:19100 >/dev/null 2>&1 & PIDS+=($!)

"$B" init server -o e2e-server.yaml >/dev/null
sed -i.bak 's/0.0.0.0:7835/127.0.0.1:27935/; s/0.0.0.0:7840/127.0.0.1:27940/; s/127.0.0.1:3000/127.0.0.1:19100/' e2e-server.yaml
case "$(uname)" in
  Darwin) SOCK_DIR=/tmp/burrow-e2e-sock ;;
  *) SOCK_DIR=/run/burrow-e2e-sock ;;
esac
SOCK="$SOCK_DIR/echo.sock"
printf '  sock:\n    client: home\n    bind: 127.0.0.1:27941\n    target: unix:%s\n' "$SOCK" >> e2e-server.yaml
printf '  db:\n    client: home\n    target: 127.0.0.1:19100\n    visitors: [laptop]\nvisitors:\n  laptop:\n    secret: %s\nmetrics:\n  listen: 127.0.0.1:27961\n' "$("$B" gensecret)" >> e2e-server.yaml
sudo mkdir -p "$SOCK_DIR"
sudo python3 -c '
import os, socket, sys, threading
path = sys.argv[1]
if os.path.exists(path):
    os.remove(path)
s = socket.socket(socket.AF_UNIX)
s.bind(path)
os.chmod(path, 0o666)
s.listen()
def echo(c):
    while True:
        data = c.recv(65536)
        if not data:
            break
        c.sendall(data)
    c.close()
while True:
    threading.Thread(target=echo, args=(s.accept()[0],), daemon=True).start()
' "$SOCK" >/dev/null 2>&1 & SOCK_PID=$!
"$B" validate -c e2e-server.yaml >/dev/null || fail "server config with a visitor is invalid: $("$B" validate -c e2e-server.yaml)"
"$B" client-config home -c e2e-server.yaml --server 127.0.0.1:27935 > e2e-client.yaml
VISITOR="$("$B" visitor-config laptop -c e2e-server.yaml --server 127.0.0.1:27935 2>/dev/null)" || fail "visitor-config: $("$B" visitor-config laptop -c e2e-server.yaml --server 127.0.0.1:27935 2>&1)"
echo "$VISITOR" | sed 's/local: 19100/local: 27950/' > e2e-visitor.yaml
SECRET="$(grep 'secret:' e2e-client.yaml | awk '{print $2}')"
sed -i.bak "s|secret: .*|secret: \${E2E_SECRET}|" e2e-client.yaml
chmod 600 e2e-server.yaml e2e-client.yaml e2e-visitor.yaml

echo "== a busy port stops the install before anything changes"
python3 -c 'import socket,time; s=socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); s.bind(("127.0.0.1",27940)); s.listen(); time.sleep(60)' & BUSY=$!
sleep 0.5
if out="$(sudo "$B" service install -c e2e-server.yaml 2>&1)"; then fail "install succeeded with a busy port"; fi
echo "$out" | grep -q "port 27940 (service \"web\") is already in use" || fail "unexpected message: $out"
sudo "$B" service list | grep -q burrow-e2e-server && fail "a failed install left a service behind"
kill "$BUSY"; wait "$BUSY" 2>/dev/null || true
check "busy port reported: $(echo "$out" | head -1)"

echo "== sudo drops variables: the install explains it"
if out="$(sudo "$B" service install -c e2e-client.yaml 2>&1)"; then fail "install succeeded without E2E_SECRET"; fi
echo "$out" | grep -q "preserve-env=E2E_SECRET" || fail "missing sudo hint: $out"
check "env hint shown"

if [ "$(uname -s)" = Linux ] && command -v openssl >/dev/null && ! id burrow >/dev/null 2>&1; then
  echo "== a failed install removes the account it created"
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout "$WORK/ca.key" -out "$WORK/ca.pem" -days 1 -subj /CN=e2e >/dev/null 2>&1
  { cat e2e-client.yaml; echo "ca: $WORK/ca.pem"; } > e2e-ca.yaml
  if out="$(E2E_SECRET="$SECRET" sudo --preserve-env=E2E_SECRET "$B" service install -c e2e-ca.yaml 2>&1)"; then fail "install succeeded with a CA the account cannot read"; fi
  echo "$out" | grep -q -E "cannot (read|use) ca" || fail "unexpected message: $out"
  id burrow >/dev/null 2>&1 && fail "the failed install left the burrow account behind"
  sudo "$B" service list | grep -q burrow-e2e-ca && fail "the failed install left a service behind"
  rm -f e2e-ca.yaml
  check "a failed install removes the account it created"
fi

echo "== install server and client"
out="$(sudo "$B" service install -c e2e-server.yaml 2>&1)" || fail "server install: $out"
echo "$out"
out="$(E2E_SECRET="$SECRET" sudo --preserve-env=E2E_SECRET "$B" service install -c e2e-client.yaml 2>&1)" || fail "client install: $out"
echo "$out"
for _ in $(seq 1 50); do "$LOADGEN" latency --addr 127.0.0.1:27940 --count 10 >/dev/null 2>&1 && break; sleep 0.2; done
"$LOADGEN" latency --addr 127.0.0.1:27940 --count 500 >/dev/null || fail "no traffic through the installed services"
check "traffic flows through the installed server and client"

for _ in $(seq 1 50); do "$LOADGEN" latency --addr 127.0.0.1:27941 --count 10 >/dev/null 2>&1 && break; sleep 0.2; done
"$LOADGEN" latency --addr 127.0.0.1:27941 --count 300 >/dev/null || fail "no traffic to the unix socket through the sandboxed client"
check "the sandboxed client reaches a unix socket target"
out="$(sudo "$B" status burrow-e2e-server 2>&1)" || fail "burrow status: $out"
echo "$out" | grep -q "home  connected" || fail "burrow status does not show the client: $out"
echo "$out" | grep -Eq "sock +tcp +127.0.0.1:27941" || fail "burrow status does not show the services: $out"
out="$("$B" status 2>&1 || true)"
echo "$out" | grep -q "belong to another user" || fail "unprivileged burrow status should point to sudo: $out"
check "burrow status reads the installed server"

running burrow-e2e-server || fail "server not running"
LOG="$(sudo "$B" service status burrow-e2e-server | sed -n 's/^ *logs: *//p' | head -1)"
sudo test -f "$LOG" || fail "no log file at $LOG"
case "$(uname)" in
  Darwin) perms="$(sudo stat -f '%Lp %Su' "$LOG")" ;;
  *) perms="$(sudo stat -c '%a %U' "$LOG")" ;;
esac
case "${perms%% *}" in
  600|640) ;;
  *) fail "log file mode is $perms; other users must not read it" ;;
esac
sudo "$B" service logs burrow-e2e-server -n 20 | grep -q "listening for clients" || fail "service logs show nothing"
out="$("$B" service logs burrow-e2e-server 2>&1 || true)"
echo "$out" | grep -q "run: sudo .*burrow service logs burrow-e2e-server" || fail "unprivileged logs should point to sudo: $out"
check "log file $LOG ($perms) and burrow service logs"
echo "== install a visitor and reach the private service"
out="$(sudo "$B" service install -c e2e-visitor.yaml 2>&1)" || fail "visitor install: $out"
echo "$out"
for _ in $(seq 1 50); do "$LOADGEN" latency --addr 127.0.0.1:27950 --count 10 >/dev/null 2>&1 && break; sleep 0.2; done
"$LOADGEN" latency --addr 127.0.0.1:27950 --count 500 >/dev/null || fail "no traffic through the installed visitor"
sudo "$B" service logs burrow-e2e-visitor -n 20 | grep -q "forwards are ready" || fail "visitor logs do not say it is ready: $(sudo "$B" service logs burrow-e2e-visitor -n 20)"
out="$(sudo "$B" service reload burrow-e2e-visitor 2>&1)" || fail "visitor reload: $out"
running burrow-e2e-visitor || fail "visitor not running after reload: $out"
for _ in $(seq 1 50); do sudo "$B" service logs burrow-e2e-visitor -n 20 | grep -q "config reloaded: nothing changed" && break; sleep 0.2; done
sudo "$B" service logs burrow-e2e-visitor -n 20 | grep -q "config reloaded: nothing changed" || fail "the visitor did not reload in place: $(sudo "$B" service logs burrow-e2e-visitor -n 20)"
check "visitor service reaches the private service"
sudo "$B" service list | grep -c "running" | grep -q 3 || fail "list does not show all three services"
out="$(sudo "$B" status burrow-e2e-client 2>&1)" && fail "status of a client service succeeded: $out"
echo "$out" | grep -q "is a client service, and burrow status shows servers only" || fail "status of a client service: $out"
check "status and list"

echo "== the same tunnel cannot also run by hand"
if out="$(E2E_SECRET="$SECRET" "$B" client -c e2e-client.yaml 2>&1)"; then fail "second client started"; fi
echo "$out" | grep -q "is already running" || fail "unexpected: $out"
check "manual duplicate refused: $(echo "$out" | head -1)"

echo "== restart and reload keep it working"
out="$(sudo "$B" service restart burrow-e2e-server 2>&1)" || fail "restart: $out"
out="$(sudo "$B" service reload burrow-e2e-client 2>&1)" || fail "reload: $out"
for _ in $(seq 1 100); do "$LOADGEN" latency --addr 127.0.0.1:27940 --count 10 >/dev/null 2>&1 && break; sleep 0.2; done
"$LOADGEN" latency --addr 127.0.0.1:27940 --count 200 >/dev/null || fail "no traffic after restart"
sudo "$B" service logs burrow-e2e-client -n 50 | grep -q "panicked\|exited" && fail "client crashed during reload"
check "restart and reload"

echo "== reinstall replaces the running service in place"
out="$(sudo "$B" service install -c e2e-server.yaml 2>&1)" || fail "reinstall: $out"
running burrow-e2e-server || fail "not running after reinstall"
check "reinstall"
cp e2e-client.yaml with-variables.yaml
sed -i.bak "s|secret: \${E2E_SECRET}|secret: $SECRET|" e2e-client.yaml
out="$(sudo "$B" service install -c e2e-client.yaml 2>&1)" || fail "reinstall without variables: $out"
sudo test -e /etc/burrow/e2e-client.env && fail "a reinstall without variables kept the old env file with the secret"
running burrow-e2e-client || fail "client not running after reinstall without variables"
mv with-variables.yaml e2e-client.yaml
out="$(E2E_SECRET="$SECRET" sudo --preserve-env=E2E_SECRET "$B" service install -c e2e-client.yaml 2>&1)" || fail "reinstall with variables: $out"
running burrow-e2e-client || fail "client not running after reinstall with variables"
check "a reinstall releases what the previous install used and the new one does not"

echo "== update: same version is left alone, --force replaces and restarts"
out="$(sudo "$B" service update 2>&1)" || fail "update: $out"
echo "$out" | grep -q "nothing to do" || fail "same-version update did something: $out"
out="$(sudo "$B" service update --force 2>&1)" || fail "forced update: $out"
echo "$out" | grep -q "restarted burrow-e2e-client, burrow-e2e-server, burrow-e2e-visitor" || fail "forced update: $out"
for _ in $(seq 1 100); do "$LOADGEN" latency --addr 127.0.0.1:27940 --count 10 >/dev/null 2>&1 && break; sleep 0.2; done
"$LOADGEN" latency --addr 127.0.0.1:27940 --count 200 >/dev/null || fail "no traffic after update"
check "update: $(echo "$out" | head -1)"

echo "== commands that restart services finish even when the session that started them is killed"
tree() {
  local child
  for child in $(pgrep -P "$1" || true); do tree "$child"; done
  case "$(ps -o args= -p "$1" 2>/dev/null || true)" in
    *--worker*) ;;
    *) echo "$1" ;;
  esac
}
interrupted() {
  local job="$1"
  shift
  "$@" > "$WORK/hup.out" 2>&1 &
  local fg=$!
  for _ in $(seq 1 500); do grep -q "in the background" "$WORK/hup.out" && break; sleep 0.02; done
  local victims
  victims="$(tree "$fg")"
  sudo kill -HUP $victims 2>/dev/null || true
  sudo kill -KILL $victims 2>/dev/null || true
  grep -q "in the background" "$WORK/hup.out" || fail "$job did not hand over to the background: $(cat "$WORK/hup.out")"
  local log
  log="$(sed -n 's/.*(log: \(.*\))$/\1/p' "$WORK/hup.out" | head -1)"
  case "$log" in */jobs/"$job"-*.log) ;; *) fail "unexpected log path for $job: $log" ;; esac
  JOBS="$(dirname "$log")"
  local how="still finishes"
  if wait "$fg"; then
    how="finished before it could be interrupted"
  else
    for _ in $(seq 1 150); do sudo grep -q $'^\x01ok' "$log" 2>/dev/null && break; sleep 0.2; done
  fi
  for s in burrow-e2e-server burrow-e2e-client burrow-e2e-visitor; do running "$s" || fail "$s is not running after the interrupted $job"; done
  if [ "$how" = "still finishes" ]; then
    sudo grep -q $'^\x01ok' "$log" || fail "the $job did not finish after its session was killed: $(sudo cat "$log" 2>/dev/null)"
  fi
  for _ in $(seq 1 100); do "$LOADGEN" latency --addr 127.0.0.1:27940 --count 10 >/dev/null 2>&1 && break; sleep 0.2; done
  "$LOADGEN" latency --addr 127.0.0.1:27940 --count 200 >/dev/null || fail "no traffic after the interrupted $job"
  check "an interrupted $job $how"
}
interrupted update sudo "$B" service update --force
interrupted restart sudo "$B" service restart burrow-e2e-client
pid_of() { sudo "$B" service status "$1" 2>/dev/null | sed -n 's/.*(pid \([0-9]*\)).*/\1/p' | head -1; }
before="$(pid_of burrow-e2e-client)"
sudo "$B" service reload burrow-e2e-client >/dev/null || fail "client reload failed"
for _ in $(seq 1 50); do sudo "$B" service logs burrow-e2e-client -n 20 | grep -q "config reloaded: nothing changed" && break; sleep 0.2; done
sudo "$B" service logs burrow-e2e-client -n 20 | grep -q "config reloaded: nothing changed" || fail "the client did not reload: $(sudo "$B" service logs burrow-e2e-client -n 20)"
[ -n "$before" ] && [ "$(pid_of burrow-e2e-client)" = "$before" ] || fail "reload restarted the client instead of reloading it"
check "reload applies the config in place, without a restart"
if [ "$(uname -s)" = Linux ]; then
  out="$(sudo "$B" edit e2e-server --editor "sed -i s/127.0.0.1:27935/127.0.0.1:443/" </dev/null 2>&1)" && fail "edit saved a port the service cannot bind: $out"
  echo "$out" | grep -q "without permission for ports below 1024" || fail "edit did not say why: $out"
  echo "$out" | grep -q "service install -c" || fail "edit did not say how: $out"
  sudo grep -q "127.0.0.1:27935" /etc/burrow/e2e-server.yaml || fail "edit changed the config anyway"
  check "edit refuses a change the installed service could not run after a restart"
fi
interrupted install env E2E_SECRET="$SECRET" sudo --preserve-env=E2E_SECRET "$B" service install -c e2e-client.yaml
[ "$(sudo stat -c %a "$JOBS" 2>/dev/null || sudo stat -f %Lp "$JOBS")" = 700 ] || fail "$JOBS is not private"
sudo grep -rqs -- "$SECRET" /var/log/burrow "$JOBS" && fail "a log contains the client secret"
sudo ls "$JOBS" | grep -q '\.env$' && fail "the variables handed to the background install were left behind"
check "no secret left in the logs"

echo "== burrow update downloads a release, verifies it and updates the services"
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) PLAT=linux-amd64 ;;
  Linux-aarch64) PLAT=linux-arm64 ;;
  Darwin-arm64) PLAT=macos-arm64 ;;
  Darwin-x86_64) PLAT=macos-amd64 ;;
esac
V="$("$B" version | awk '{print $2}')"
REL="$WORK/mirror/releases"
PKG="burrow-$V-$PLAT"
mkdir -p "$REL/download/v$V/$PKG" "$REL/latest/download"
cp "$B" "$REL/download/v$V/$PKG/burrow"
printf 'e2e build' >> "$REL/download/v$V/$PKG/burrow"
tar -C "$REL/download/v$V" -czf "$REL/download/v$V/$PKG.tar.gz" "$PKG"
(cd "$REL/download/v$V" && shasum -a 256 "$PKG.tar.gz" > SHA256SUMS)
cp "$REL/download/v$V/SHA256SUMS" "$REL/latest/download/SHA256SUMS"
python3 -m http.server 27960 --bind 127.0.0.1 --directory "$WORK/mirror" >/dev/null 2>&1 & PIDS+=($!)
MIRROR=http://127.0.0.1:27960/releases
for _ in $(seq 1 50); do curl -fs "$MIRROR/latest/download/SHA256SUMS" >/dev/null && break; sleep 0.1; done
cp "$B" "$WORK/burrow"
out="$(BURROW_UPDATE_URL="$MIRROR" "$WORK/burrow" update --check 2>&1)" || fail "update --check: $out"
echo "$out" | grep -q "burrow $V is up to date" || fail "update --check: $out"
out="$(BURROW_UPDATE_URL="$MIRROR" "$WORK/burrow" update --force 2>&1)" || fail "unprivileged update: $out"
echo "$out" | grep -q "the installed services still use" || fail "unprivileged update does not point to the services: $out"
cmp -s "$WORK/burrow" "$REL/download/v$V/$PKG/burrow" || fail "unprivileged update did not replace its own binary"
cp "$B" "$WORK/burrow"
out="$(sudo env BURROW_UPDATE_URL="$MIRROR" "$WORK/burrow" update --force 2>&1)" || fail "update: $out"
echo "$out"
echo "$out" | grep -q "restarted burrow-e2e-client, burrow-e2e-server, burrow-e2e-visitor" || fail "update did not restart the services: $out"
INSTALLED="$(sudo "$B" service status burrow-e2e-server | sed -n 's/^ *binary: *\(.*\) (.*/\1/p' | head -1)"
sudo cmp -s "$INSTALLED" "$REL/download/v$V/$PKG/burrow" || fail "the services do not run the downloaded binary"
cmp -s "$WORK/burrow" "$REL/download/v$V/$PKG/burrow" || fail "update did not replace its own binary"
for _ in $(seq 1 100); do "$LOADGEN" latency --addr 127.0.0.1:27940 --count 10 >/dev/null 2>&1 && break; sleep 0.2; done
"$LOADGEN" latency --addr 127.0.0.1:27940 --count 200 >/dev/null || fail "no traffic after burrow update"
"$LOADGEN" latency --addr 127.0.0.1:27950 --count 200 >/dev/null || fail "no visitor traffic after burrow update"
ls -a "$WORK" | grep -q burrow-update && fail "burrow update left its download folder behind"
check "burrow update replaced the installed binary and restarted every service"

echo "== a broken config is explained in status and logs"
CFG="$(sudo "$B" service status burrow-e2e-client | sed -n 's/^ *config: *//p' | sed 's/  (missing!)$//' | head -1)"
sudo cp "$CFG" "$WORK/good.yaml"
echo "bogus_field: 1" | sudo tee -a "$CFG" >/dev/null
sudo "$B" service restart burrow-e2e-client >/dev/null 2>&1 || true
sleep 3
st="$(sudo "$B" service status burrow-e2e-client || true)"
echo "$st" | grep -q "config has errors" || fail "status does not explain the broken config: $st"
sudo "$B" service logs burrow-e2e-client -n 20 | grep -q "not starting" || fail "the logs do not say why it stopped"
sudo cp "$WORK/good.yaml" "$CFG"
sudo "$B" service start burrow-e2e-client >/dev/null
running burrow-e2e-client || fail "not running after fixing the config"
check "broken config reported, fixed config runs again"

echo "== a server with an admin visitor may change its own config, under the sandbox"
OPS="$("$B" gensecret)"
"$B" init server -o e2e-admin.yaml >/dev/null
sed -i.bak 's/0.0.0.0:7835/127.0.0.1:27936/; s/0.0.0.0:7840/127.0.0.1:27946/' e2e-admin.yaml
printf 'visitors:\n  ops:\n    secret: %s\n    admin: write\n' "$OPS" >> e2e-admin.yaml
chmod 600 e2e-admin.yaml
printf 'server: 127.0.0.1:27936\nname: ops\nsecret: %s\nforwards: {}\n' "$OPS" > e2e-ops.yaml
chmod 600 e2e-ops.yaml
out="$(sudo "$B" service install -c e2e-admin.yaml 2>&1)" || fail "admin server install: $out"
echo "$out" | grep -q "may change its config in /etc/burrow/e2e-admin because visitor ops has admin access" || fail "install does not say the config is writable: $out"
running burrow-e2e-admin || fail "admin server not running"
if [ "$(uname -s)" = Linux ]; then
  sudo grep -q "^ReadWritePaths=/etc/burrow/e2e-admin$" /etc/systemd/system/burrow-e2e-admin.service || fail "the unit does not let the server write its config folder"
  ACCOUNT="$(sudo stat -c '%U' /etc/burrow/e2e-admin/server.yaml)"
  [ "$ACCOUNT" != root ] || fail "the installed config belongs to root"
  [ "$(sudo stat -c '%a %U' /etc/burrow/e2e-admin)" = "700 $ACCOUNT" ] || fail "the config folder is not private to the service account $ACCOUNT: $(sudo stat -c '%a %U' /etc/burrow/e2e-admin)"
fi
for _ in $(seq 1 50); do "$PROBE" e2e-ops.yaml state >/dev/null 2>&1 && break; sleep 0.2; done
state="$("$PROBE" e2e-ops.yaml state 2>&1)" || fail "admin state: $state"
echo "$state" | grep -q '"writable":true' || fail "the installed server says it cannot write its config: $state"
out="$("$PROBE" e2e-ops.yaml apply '[{"op":"add","path":["services","added"],"value":{"client":"home","bind":"127.0.0.1:27947","target":9}}]' 2>&1)" || fail "admin apply: $out"
for _ in $(seq 1 50); do sudo "$B" status burrow-e2e-admin 2>/dev/null | grep -q "added" && break; sleep 0.2; done
sudo "$B" status burrow-e2e-admin | grep -Eq "added +tcp +127.0.0.1:27947" || fail "the installed server did not reload the admin change: $(sudo "$B" status burrow-e2e-admin 2>&1)"
sudo grep -q "added:" /etc/burrow/e2e-admin/server.yaml || fail "the change is not in the installed config"
[ "$(sudo ls /etc/burrow/e2e-admin/.server.yaml.history | wc -l)" -ge 1 ] || fail "no earlier version was kept"
sudo "$B" service logs burrow-e2e-admin -n 30 | grep -q "config changed by admin" || fail "the change is not in the service log"
check "the sandboxed server applied a change made through the admin channel"
if [ "$(uname -s)" = Linux ]; then
  cat > "$WORK/add-admin.py" <<'PY'
import sys
secret, path = sys.argv[1], sys.argv[2]
text = open(path).read().replace("visitors:\n", "visitors:\n  watcher:\n    secret: %s\n    admin: read\n" % secret, 1)
open(path, "w").write(text)
PY
  WATCH="$("$B" gensecret)"
  out="$(sudo "$B" edit e2e-server --editor "python3 $WORK/add-admin.py $WATCH" </dev/null 2>&1)" || true
  sudo grep -q "watcher:" /etc/burrow/e2e-server.yaml || fail "could not add an admin visitor by hand: $out"
  echo "$out" | grep -q "installed without permission to change its config" || fail "edit does not warn that admin will be read-only: $out"
  printf 'server: 127.0.0.1:27935\nname: watcher\nsecret: %s\nforwards: {}\n' "$WATCH" > e2e-watcher.yaml
  chmod 600 e2e-watcher.yaml
  for _ in $(seq 1 50); do "$PROBE" e2e-watcher.yaml state >/dev/null 2>&1 && break; sleep 0.2; done
  state="$("$PROBE" e2e-watcher.yaml state 2>&1)" || fail "read-only admin state: $state"
  echo "$state" | grep -q '"writable":false' || fail "a server that cannot write its config claims it can: $state"
  echo "$state" | grep -q '"code":"read-only-fs"' || fail "the read-only reason is not the sandbox: $state"
  check "a server installed without admin says honestly that its config is read-only"
fi
sudo "$B" service uninstall burrow-e2e-admin --purge >/dev/null || fail "admin server uninstall"
[ -e /etc/burrow/e2e-admin ] && fail "/etc/burrow/e2e-admin still exists"
check "uninstall --purge removes the config folder and its versions"

echo "== uninstall removes everything it created"
sudo "$B" service uninstall burrow-e2e-server --purge
sudo "$B" service uninstall burrow-e2e-client --purge
sudo "$B" service uninstall burrow-e2e-visitor --purge
sudo "$B" service list | grep -q burrow-e2e && fail "services still listed"
for f in "$STATE" /etc/burrow/e2e-server.yaml /etc/burrow/e2e-client.yaml /etc/burrow/e2e-client.env /etc/burrow/e2e-visitor.yaml /usr/local/bin/burrow /usr/local/bin/burrow.previous "$LOG" "$JOBS"; do
  [ -e "$f" ] && fail "$f still exists"
done
case "$(uname -s)" in
  Darwin) [ -e "/Library/Application Support/burrow" ] && fail "/Library/Application Support/burrow still exists" ;;
  Linux) [ -e /run/burrow-instances ] && fail "/run/burrow-instances still exists" ;;
esac
if id burrow >/dev/null 2>&1 || id _burrow >/dev/null 2>&1; then fail "service account still exists"; fi
sudo "$B" service uninstall burrow-e2e-server --purge >/dev/null 2>&1 && fail "uninstalling a missing service succeeded"
[ -e "$STATE" ] && fail "uninstalling a missing service created $STATE"
check "nothing left behind"
echo "service end-to-end checks passed"
