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
  if [ "$(uname -s)" = Linux ]; then
    sudo "$B" service uninstall burrow-e2e-oldunit --purge >/dev/null 2>&1
    sudo systemctl disable --now burrow-e2e-oldunit burrow-e2e-other >/dev/null 2>&1
    sudo rm -f /etc/systemd/system/burrow-e2e-oldunit.service /etc/systemd/system/burrow-e2e-other.service \
      "$STATE/services/burrow-e2e-oldunit.json" /etc/burrow/e2e-oldunit.yaml /etc/burrow/e2e-oldunit.env
    sudo systemctl daemon-reload
  fi
  sudo "$B" service uninstall burrow-e2e-visitor --purge >/dev/null 2>&1
  sudo "$B" service uninstall burrow-e2e-admin --purge >/dev/null 2>&1
  sudo "$B" service uninstall burrow-e2e-inplace --purge >/dev/null 2>&1
  sudo rm -rf /etc/burrow/e2e-inplace /etc/burrow/e2e-inplace.yaml.bak
  sudo "$B" service uninstall burrow-e2e-rootadmin --purge >/dev/null 2>&1
  sudo rm -rf /etc/burrow/e2e-rootadmin
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
mode_owner() { sudo stat -c '%a %U' "$1" 2>/dev/null || sudo stat -f '%Lp %Su' "$1"; }
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
sudo mkdir -p -m 755 "$SOCK_DIR"
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
PIDS+=("$BUSY")
for _ in $(seq 1 100); do (exec 3<>/dev/tcp/127.0.0.1/27940) 2>/dev/null && break; sleep 0.1; done
(exec 3<>/dev/tcp/127.0.0.1/27940) 2>/dev/null || fail "the busy-port listener did not start within 10 s"
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

if [ "$(uname -s)" = Linux ]; then
  echo "== a burrow unit burrow has no record of is named, and taken over only once stopped"
  BIN="$(sudo "$B" service status burrow-e2e-server | sed -n 's/^ *binary: *\([^ ]*\).*/\1/p')"
  "$B" init server -o e2e-oldunit.yaml >/dev/null
  sed -i.bak 's/0.0.0.0:7835/127.0.0.1:27936/; s/0.0.0.0:7840/127.0.0.1:27946/' e2e-oldunit.yaml
  ACCOUNT="$(sudo "$B" service status burrow-e2e-server | sed -n 's/^ *account: *//p')"
  sudo install -m 600 -o "$ACCOUNT" e2e-oldunit.yaml /etc/burrow/e2e-oldunit.yaml
  printf 'OLD_SECRET="x"\n' | sudo install -m 600 -o "$ACCOUNT" /dev/stdin /etc/burrow/e2e-oldunit.env
  printf '[Service]\nUser=%s\nExecStart=%s server -c /etc/burrow/e2e-oldunit.yaml\n[Install]\nWantedBy=multi-user.target\n' "$ACCOUNT" "$BIN" \
    | sudo tee /etc/systemd/system/burrow-e2e-oldunit.service >/dev/null
  printf '{"name":"burrow-e2e-oldunit","role":"server","config":"/etc/burrow/e2e-oldunit.yaml","source_config":"/root/e2e-oldunit.yaml","env_file":"/etc/burrow/e2e-oldunit.env","log_file":"","binary":"%s","copied_binary":true,"account":"root","created_account":false,"low_ports":false,"files":[],"version":"0.1.0"}\n' "$BIN" \
    | sudo tee "$STATE/services/burrow-e2e-oldunit.json" >/dev/null
  printf '[Service]\nExecStart=/bin/sleep 100000\n' | sudo tee /etc/systemd/system/burrow-e2e-other.service >/dev/null
  sudo systemctl daemon-reload
  sudo systemctl start burrow-e2e-oldunit burrow-e2e-other
  for _ in $(seq 1 50); do (exec 3<>/dev/tcp/127.0.0.1/27936) 2>/dev/null && break; sleep 0.2; done
  OLDPID="$(systemctl show -p MainPID --value burrow-e2e-oldunit)"
  out="$(sudo "$B" service list 2>&1)"
  echo "$out" | grep -Eq "^burrow-e2e-oldunit +\? +running +not managed by this burrow" || fail "list does not name the unit: $out"
  echo "$out" | grep -q "burrow-e2e-other" && fail "list claims a unit that does not run burrow: $out"
  out="$(sudo "$B" service status burrow-e2e-oldunit 2>&1)" && fail "status of an unmanaged unit succeeded: $out"
  echo "$out" | grep -q "burrow-e2e-oldunit.service is running, but burrow has no usable record of it" || fail "status: $out"
  echo "$out" | grep -q "take it over: sudo systemctl stop burrow-e2e-oldunit, then sudo .*service install -c /etc/burrow/e2e-oldunit.yaml$" || fail "status hint: $out"
  out="$(sudo "$B" service update 2>&1)" || fail "update: $out"
  echo "$out" | grep -q "note: burrow-e2e-oldunit.service (running) was not restarted" || fail "update does not name the unit: $out"
  if out="$(sudo "$B" service install -c /etc/burrow/e2e-oldunit.yaml 2>&1)"; then fail "install over a running unmanaged unit succeeded: $out"; fi
  echo "$out" | grep -q "burrow-e2e-oldunit.service is running, but burrow has no usable record of it" || fail "install refusal: $out"
  [ "$(systemctl show -p MainPID --value burrow-e2e-oldunit)" = "$OLDPID" ] || fail "the refused install restarted the unit"
  grep -q "/etc/burrow/e2e-oldunit.yaml" /etc/systemd/system/burrow-e2e-oldunit.service || fail "the refused install changed the unit"
  sudo systemctl stop burrow-e2e-oldunit
  out="$(sudo "$B" service install -c /etc/burrow/e2e-oldunit.yaml 2>&1)" || fail "install over a stopped unmanaged unit: $out"
  echo "$out" | grep -q "note: replaced burrow-e2e-oldunit.service, which burrow had no record of; its old config was left as it was, now owned by root: /etc/burrow/e2e-oldunit.yaml" || fail "takeover note: $out"
  echo "$out" | grep -q "note: gave /etc/burrow/e2e-oldunit.env, left by an older install of burrow-e2e-oldunit, to root" || fail "takeover env note: $out"
  for f in /etc/burrow/e2e-oldunit.yaml /etc/burrow/e2e-oldunit.env; do
    [ "$(sudo stat -c %u "$f")" = 0 ] || fail "the takeover left $f owned by uid $(sudo stat -c %u "$f")"
  done
  echo "$out" | grep -q "started burrow-e2e-oldunit" || fail "takeover did not start: $out"
  running burrow-e2e-oldunit || fail "the taken-over unit does not run"
  sudo "$B" service list | grep -Eq "^burrow-e2e-oldunit +server +running +/etc/burrow/e2e-oldunit/server.yaml" || fail "list after takeover: $(sudo "$B" service list)"
  sudo "$B" service uninstall burrow-e2e-oldunit --purge >/dev/null || fail "uninstall of the taken-over unit"
  sudo systemctl stop burrow-e2e-other
  sudo rm -f /etc/systemd/system/burrow-e2e-other.service /etc/burrow/e2e-oldunit.env
  sudo systemctl daemon-reload
  check "an unmanaged unit is named, refused while running, and taken over once stopped"
fi

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
  sudo grep -q "127.0.0.1:27935" /etc/burrow/e2e-server/server.yaml || fail "edit changed the config anyway"
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
st="$(sudo "$B" service status burrow-e2e-admin 2>&1)" || true
echo "$st" | grep -q "is managed by burrow admin" || fail "status does not say the admin manages the installed config: $st"
echo "$st" | grep -q "to use it, run" && fail "status suggests a reinstall that would undo the admin change: $st"
out="$(sudo "$B" service install -c e2e-admin.yaml 2>&1)" || fail "admin server reinstall: $out"
echo "$out" | grep -q "kept the installed config as a history version before replacing it" || fail "the reinstall does not say it kept the installed config: $out"
sudo sh -c 'grep -l "added:" /etc/burrow/e2e-admin/.server.yaml.history/*' >/dev/null || fail "the admin change is not among the kept versions"
running burrow-e2e-admin || fail "reinstalled admin server not running"
check "a reinstall keeps the installed config as a version to revert to"
if [ "$(uname -s)" = Linux ]; then
  cat > "$WORK/add-admin.py" <<'PY'
import sys
secret, path = sys.argv[1], sys.argv[2]
text = open(path).read().replace("visitors:\n", "visitors:\n  watcher:\n    secret: %s\n    admin: read\n" % secret, 1)
open(path, "w").write(text)
PY
  WATCH="$("$B" gensecret)"
  out="$(sudo "$B" edit e2e-server --editor "python3 $WORK/add-admin.py $WATCH" </dev/null 2>&1)" || true
  sudo grep -q "watcher:" /etc/burrow/e2e-server/server.yaml || fail "could not add an admin visitor by hand: $out"
  echo "$out" | grep -q "installed without permission to change its config" || fail "edit does not warn that admin will be read-only: $out"
  echo "$out" | grep -q "service install -c /etc/burrow/e2e-server/server.yaml --admin-writable" || fail "edit does not name the command that makes the installed config writable: $out"
  printf 'server: 127.0.0.1:27935\nname: watcher\nsecret: %s\nforwards: {}\n' "$WATCH" > e2e-watcher.yaml
  chmod 600 e2e-watcher.yaml
  for _ in $(seq 1 50); do "$PROBE" e2e-watcher.yaml state >/dev/null 2>&1 && break; sleep 0.2; done
  state="$("$PROBE" e2e-watcher.yaml state 2>&1)" || fail "read-only admin state: $state"
  echo "$state" | grep -q '"writable":false' || fail "a server that cannot write its config claims it can: $state"
  echo "$state" | grep -q '"code":"read-only-fs"' || fail "the read-only reason is not the sandbox: $state"
  echo "$state" | grep -q 'service install -c /etc/burrow/e2e-server/server.yaml --admin-writable' || fail "the admin read-only hint does not name the exact command: $state"
  check "a server installed without admin says honestly that its config is read-only"
fi

echo "== admin writes switch on and off from the installed config, and nothing moves"
FOLDER=/etc/burrow/e2e-inplace
INSTALLED="$FOLDER/server.yaml"
mkdir -p download
"$B" init server -o download/e2e-inplace.yaml >/dev/null
sed -i.bak 's/0.0.0.0:7835/127.0.0.1:27937/; s/0.0.0.0:7840/127.0.0.1:27948/' download/e2e-inplace.yaml
chmod 600 download/e2e-inplace.yaml
out="$(sudo "$B" service install -c download/e2e-inplace.yaml 2>&1)" || fail "read-only install: $out"
rm -rf download
[ "$(mode_owner "$FOLDER")" = "755 root" ] || fail "the read-only config folder is not root's: $(mode_owner "$FOLDER")"
INPLACE_ACCOUNT="$(mode_owner "$INSTALLED" | awk '{print $2}')"
[ "$(mode_owner "$INSTALLED")" = "600 $INPLACE_ACCOUNT" ] || fail "the installed config is not private: $(mode_owner "$INSTALLED")"
printf 'visitors:\n  ops:\n    secret: %s\n    admin: write\n' "$OPS" | sudo tee -a "$INSTALLED" >/dev/null
out="$(sudo "$B" service install --admin-writable -c "$INSTALLED" 2>&1)" || fail "admin-writable reinstall from the installed config: $out"
echo "$out" | grep -qi "delete" && fail "switching admin writes on asks to delete something: $out"
running burrow-e2e-inplace || fail "the server does not run after switching admin writes on"
[ "$(mode_owner "$FOLDER")" = "700 $INPLACE_ACCOUNT" ] || fail "the writable config folder is not the service account's: $(mode_owner "$FOLDER")"
[ "$(sudo find "$FOLDER" -maxdepth 1 -type f | wc -l | tr -d ' ')" = 1 ] || fail "switching admin writes on left another file: $(sudo ls -la "$FOLDER")"
sudo grep -q "ops:" "$INSTALLED" || fail "the installed config lost the admin visitor"
st="$(sudo "$B" service status burrow-e2e-inplace 2>&1)" || true
echo "$st" | grep -q "config:  $INSTALLED (the service may change it" || fail "status does not say the service may change its config: $st"
printf 'server: 127.0.0.1:27937\nname: ops\nsecret: %s\nforwards: {}\n' "$OPS" > e2e-inplace-ops.yaml
chmod 600 e2e-inplace-ops.yaml
for _ in $(seq 1 50); do "$PROBE" e2e-inplace-ops.yaml state >/dev/null 2>&1 && break; sleep 0.2; done
out="$("$PROBE" e2e-inplace-ops.yaml apply '[{"op":"add","path":["services","added"],"value":{"client":"home","bind":"127.0.0.1:27933","target":9}}]' 2>&1)" || fail "admin apply on the switched server: $out"
for _ in $(seq 1 50); do sudo sh -c "ls $FOLDER/.server.yaml.history/* >/dev/null 2>&1" && break; sleep 0.2; done
sudo grep -q "added:" "$INSTALLED" || fail "the switched server did not write its installed config"
sudo perl -0pi -e 's/\nvisitors:.*//s' "$INSTALLED"
out="$(sudo "$B" service install -c "$INSTALLED" 2>&1)" || fail "read-only reinstall from the installed config: $out"
echo "$out" | grep -qi "delete" && fail "switching admin writes off asks to delete something: $out"
running burrow-e2e-inplace || fail "the server does not run after switching admin writes off"
[ "$(mode_owner "$FOLDER")" = "755 root" ] || fail "switching admin writes off left the folder with the service account: $(mode_owner "$FOLDER")"
left="$(sudo find "$FOLDER" -mindepth 1 ! -uid 0 ! -path "$INSTALLED")"
[ -z "$left" ] || fail "switching admin writes off left files with the service account: $left"
[ "$(sudo find "$FOLDER" -maxdepth 1 -type f | wc -l | tr -d ' ')" = 1 ] || fail "switching admin writes off left another file: $(sudo ls -la "$FOLDER")"
[ "$(sudo ls "$FOLDER/.server.yaml.history" | wc -l)" -ge 1 ] || fail "switching admin writes off lost the kept versions"
out="$(sudo "$B" service uninstall burrow-e2e-inplace --purge 2>&1)" || fail "uninstall: $out"
sudo test -e "$FOLDER" && fail "uninstall --purge left $FOLDER"
check "admin writes switch on and off in place, with no source left and nothing to delete"

if [ "$(uname -s)" = Linux ]; then
  echo "== a server run as root keeps its admin history under the systemd filter"
  "$B" init server -o e2e-rootadmin.yaml >/dev/null
  sed -i.bak 's/0.0.0.0:7835/127.0.0.1:27938/; s/0.0.0.0:7840/127.0.0.1:27949/' e2e-rootadmin.yaml
  printf 'visitors:\n  ops:\n    secret: %s\n    admin: write\n' "$OPS" >> e2e-rootadmin.yaml
  chmod 600 e2e-rootadmin.yaml
  printf 'server: 127.0.0.1:27938\nname: ops\nsecret: %s\nforwards: {}\n' "$OPS" > e2e-rootops.yaml
  chmod 600 e2e-rootops.yaml
  out="$(sudo "$B" service install -c e2e-rootadmin.yaml --run-as root 2>&1)" || fail "root admin server install: $out"
  running burrow-e2e-rootadmin || fail "root admin server not running"
  for _ in $(seq 1 50); do "$PROBE" e2e-rootops.yaml state >/dev/null 2>&1 && break; sleep 0.2; done
  out="$("$PROBE" e2e-rootops.yaml apply '[{"op":"add","path":["services","added"],"value":{"client":"home","bind":"127.0.0.1:27939","target":9}}]' 2>&1)" || fail "root admin apply: $out"
  for _ in $(seq 1 50); do sudo sh -c 'ls /etc/burrow/e2e-rootadmin/.server.yaml.history/* >/dev/null 2>&1' && break; sleep 0.2; done
  [ "$(sudo ls /etc/burrow/e2e-rootadmin/.server.yaml.history | wc -l)" -ge 1 ] || fail "a root server kept no version on disk"
  state="$("$PROBE" e2e-rootops.yaml state 2>&1)" || fail "root admin state: $state"
  echo "$state" | grep -q '"why_not_history":"' && fail "a root server says it cannot keep its history: $state"
  sudo "$B" service uninstall burrow-e2e-rootadmin --purge >/dev/null || fail "root admin server uninstall"
  [ -e /etc/burrow/e2e-rootadmin ] && fail "/etc/burrow/e2e-rootadmin still exists"
  check "a root server keeps its admin history on disk"
fi

echo "== uninstall removes everything it created"
sudo "$B" service uninstall burrow-e2e-server --purge
sudo "$B" service uninstall burrow-e2e-client --purge
sudo "$B" service uninstall burrow-e2e-visitor --purge
out="$(sudo "$B" service uninstall burrow-e2e-admin 2>&1)" || fail "admin server uninstall: $out"
echo "$out" | grep -q '"/etc/burrow/e2e-admin"' || fail "the delete hint does not name the kept config folder: $out"
[ "$(sudo ls /etc/burrow/e2e-admin/.server.yaml.history | wc -l)" -ge 1 ] || fail "uninstall without --purge lost the kept versions"
left="$(sudo find /etc/burrow/e2e-admin ! -uid 0)"
[ -z "$left" ] || fail "uninstall left files of the removed account behind: $left"
check "uninstall gives the kept config folder and its versions back to root"
out="$(sudo "$B" service install -c e2e-admin.yaml 2>&1)" || fail "admin server reinstall over the kept folder: $out"
running burrow-e2e-admin || fail "reinstalled admin server not running"
left="$(sudo find /etc/burrow/e2e-admin -uid 0)"
[ -z "$left" ] || fail "the reinstalled service account does not own its kept versions: $left"
check "a reinstall gives the kept versions to the new service account"
cp e2e-admin.yaml e2e-admin.writable
sed -i.bak '/^visitors:/,$d' e2e-admin.yaml
out="$(sudo "$B" service install -c e2e-admin.yaml 2>&1)" || fail "read-only reinstall of the admin server: $out"
echo "$out" | grep -qi "delete" && fail "a reinstall without admin asks to delete something: $out"
[ "$(mode_owner /etc/burrow/e2e-admin)" = "755 root" ] || fail "a reinstall without admin left the config folder with the service account: $(mode_owner /etc/burrow/e2e-admin)"
left="$(sudo find /etc/burrow/e2e-admin -mindepth 1 ! -uid 0 ! -path /etc/burrow/e2e-admin/server.yaml)"
[ -z "$left" ] || fail "a reinstall without admin left the kept versions with the service account: $left"
check "a reinstall that turns admin writes off gives the config folder and its versions to root"
cp e2e-admin.writable e2e-admin.yaml
out="$(sudo "$B" service install -c e2e-admin.yaml 2>&1)" || fail "writable reinstall of the admin server: $out"
running burrow-e2e-admin || fail "admin server not running after turning admin writes on again"
echo "$out" | grep -qi "delete" && fail "a reinstall with admin asks to delete something: $out"
[ "$(sudo find /etc/burrow/e2e-admin -maxdepth 1 -type f | wc -l | tr -d ' ')" = 1 ] || fail "turning admin writes on left a second config: $(sudo ls -la /etc/burrow/e2e-admin)"
left="$(sudo find /etc/burrow/e2e-admin -uid 0)"
[ -z "$left" ] || fail "turning admin writes on left files with root: $left"
check "a reinstall that turns admin writes on gives the same folder back to the service account"
sudo "$B" service uninstall burrow-e2e-admin --purge >/dev/null || fail "admin server uninstall"
[ -e /etc/burrow/e2e-admin ] && fail "/etc/burrow/e2e-admin still exists"
check "uninstall --purge removes the config folder and its versions"
sudo "$B" service list | grep -q burrow-e2e && fail "services still listed"
for f in "$STATE" /etc/burrow/e2e-server /etc/burrow/e2e-client /etc/burrow/e2e-client.env /etc/burrow/e2e-visitor /usr/local/bin/burrow /usr/local/bin/burrow.previous "$LOG" "$JOBS"; do
  [ -e "$f" ] && fail "$f still exists"
done
case "$(uname -s)" in
  Darwin) [ -e "/Library/Application Support/burrow" ] && fail "/Library/Application Support/burrow still exists" ;;
  Linux) [ -e /run/burrow-instances ] && fail "/run/burrow-instances still exists" ;;
esac
if id burrow >/dev/null 2>&1 || id _burrow >/dev/null 2>&1; then fail "service account still exists"; fi
if [ "$(uname -s)" = Linux ]; then
  [ "$(sudo stat -c %u /etc/burrow/e2e-oldunit.yaml)" = 0 ] || fail "a taken-over unit's old config is owned by a removed account"
  sudo rm -f /etc/burrow/e2e-oldunit.yaml
  sudo rmdir /etc/burrow 2>/dev/null || true
fi
sudo "$B" service uninstall burrow-e2e-server --purge >/dev/null 2>&1 && fail "uninstalling a missing service succeeded"
[ -e "$STATE" ] && fail "uninstalling a missing service created $STATE"
check "nothing left behind"
echo "service end-to-end checks passed"
