#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PROFILE="${BURROW_E2E_PROFILE:-release}"
cargo build --profile "$PROFILE" --manifest-path "$ROOT/Cargo.toml" --bin burrow --example loadgen -q
OUT="$ROOT/target/$([ "$PROFILE" = dev ] && echo debug || echo "$PROFILE")"
B="$OUT/burrow"
LOADGEN="$OUT/examples/loadgen"
case "$(uname)" in
  Darwin) STATE="/Library/Application Support/burrow/services" ;;
  *) STATE=/var/lib/burrow ;;
esac
WORK="$(mktemp -d)"
PIDS=()

cleanup() {
  set +e
  sudo "$B" service uninstall burrow-e2e-client --purge >/dev/null 2>&1
  sudo "$B" service uninstall burrow-e2e-server --purge >/dev/null 2>&1
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null; done
  rm -rf "$WORK"
}
trap cleanup EXIT
[ -n "${GITHUB_ACTIONS:-}" ] && trap 'echo "::error title=service e2e::line $LINENO failed: $BASH_COMMAND"' ERR

fail() {
  echo "FAIL: $*" >&2
  if [ -n "${GITHUB_ACTIONS:-}" ]; then
    echo "::error title=service e2e::$(printf '%s' "$*" | tr '\n' ' ')"
    for n in burrow-e2e-server burrow-e2e-client; do
      echo "::error title=$n logs::$(sudo "$B" service logs "$n" -n 15 2>&1 | tr '\n' '|')"
    done
  fi
  sudo "$B" service logs burrow-e2e-server -n 30 >&2 || true; sudo "$B" service logs burrow-e2e-client -n 30 >&2 || true; exit 1; }
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
"$B" client-config home -c e2e-server.yaml --server 127.0.0.1:27935 > e2e-client.yaml
SECRET="$(grep 'secret:' e2e-client.yaml | awk '{print $2}')"
sed -i.bak "s|secret: .*|secret: \${E2E_SECRET}|" e2e-client.yaml
chmod 600 e2e-server.yaml e2e-client.yaml

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

echo "== install server and client"
out="$(sudo "$B" service install -c e2e-server.yaml 2>&1)" || fail "server install: $out"
echo "$out"
out="$(E2E_SECRET="$SECRET" sudo --preserve-env=E2E_SECRET "$B" service install -c e2e-client.yaml 2>&1)" || fail "client install: $out"
echo "$out"
for _ in $(seq 1 50); do "$LOADGEN" latency --addr 127.0.0.1:27940 --count 10 >/dev/null 2>&1 && break; sleep 0.2; done
"$LOADGEN" latency --addr 127.0.0.1:27940 --count 500 >/dev/null || fail "no traffic through the installed services"
check "traffic flows through the installed server and client"

running burrow-e2e-server || fail "server not running"
LOG="$(sudo "$B" service status burrow-e2e-server | awk '/logs:/ {print $2}')"
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
sudo "$B" service list | grep -c "running" | grep -q 2 || fail "list does not show both services"
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

echo "== update: same version is left alone, --force replaces and restarts"
out="$(sudo "$B" service update 2>&1)" || fail "update: $out"
echo "$out" | grep -q "nothing to do" || fail "same-version update did something: $out"
out="$(sudo "$B" service update --force 2>&1)" || fail "forced update: $out"
echo "$out" | grep -q "restarted burrow-e2e-client, burrow-e2e-server" || fail "forced update: $out"
for _ in $(seq 1 100); do "$LOADGEN" latency --addr 127.0.0.1:27940 --count 10 >/dev/null 2>&1 && break; sleep 0.2; done
"$LOADGEN" latency --addr 127.0.0.1:27940 --count 200 >/dev/null || fail "no traffic after update"
check "update: $(echo "$out" | head -1)"

echo "== a broken config is explained in status and logs"
CFG="$(sudo "$B" service status burrow-e2e-client | awk '/config:/ {print $2}')"
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

echo "== uninstall removes everything it created"
sudo "$B" service uninstall burrow-e2e-client --purge
sudo "$B" service uninstall burrow-e2e-server --purge
sudo "$B" service list | grep -q burrow-e2e && fail "services still listed"
for f in "$STATE" /etc/burrow/e2e-server.yaml /etc/burrow/e2e-client.yaml /etc/burrow/e2e-client.env /usr/local/bin/burrow /usr/local/bin/burrow.previous "$LOG"; do
  [ -e "$f" ] && fail "$f still exists"
done
if id burrow >/dev/null 2>&1 || id _burrow >/dev/null 2>&1; then fail "service account still exists"; fi
check "nothing left behind"
echo "service end-to-end checks passed"
