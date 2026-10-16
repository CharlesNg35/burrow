#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BENCH_DIR="${BENCH_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/burrow-bench}"
TOOLS="${TOOLS:-direct burrow burrow-aesgcm burrow-plaintext rathole rathole-noise frp frp-plain frp-nomux chisel}"
IPERF_SECS="${IPERF_SECS:-10}"
CONNRATE_SECS="${CONNRATE_SECS:-10}"
IDLE_COUNTS="${IDLE_COUNTS:-1000 10000}"
RUN="$BENCH_DIR/runs/$(date -u +%Y%m%dT%H%M%SZ)"
BIN="$BENCH_DIR/bin"
ECHO_PORT=9000
IPERF_PORT=5201
mkdir -p "$RUN"
ulimit -n "$(ulimit -Hn)"

for c in iperf3 python3 curl; do command -v "$c" >/dev/null || { echo "missing $c" >&2; exit 1; }; done
[ -x "$BIN/rathole" ] || "$ROOT/bench/fetch.sh"

cargo build --release --manifest-path "$ROOT/Cargo.toml" --example loadgen --bin burrow -q ${BURROW_FEATURES:+--features "$BURROW_FEATURES"}
BURROW="$ROOT/target/release/burrow"
LOADGEN="$ROOT/target/release/examples/loadgen"

PIDS=()
cleanup() { for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null || true; done; wait 2>/dev/null || true; PIDS=(); }
trap cleanup EXIT

start() { "$@" >>"$RUN/processes.log" 2>&1 & PIDS+=($!); }

rss_kib() {
  local total=0 p
  for p in "$@"; do total=$((total + $(awk '/VmRSS/ {print $2}' "/proc/$p/status" 2>/dev/null || echo 0))); done
  echo "$total"
}

wait_echo() {
  for _ in $(seq 1 100); do
    if "$LOADGEN" latency --addr "127.0.0.1:$1" --count 10 >/dev/null 2>&1; then return 0; fi
    sleep 0.2
  done
  echo "tunnel on port $1 did not come up" >&2
  return 1
}

setup() {
  local tool="$1" dir="$RUN/$1"
  mkdir -p "$dir"
  TUNNEL_PIDS=()
  case "$tool" in
    direct)
      EP=$ECHO_PORT; IP=$IPERF_PORT; return ;;
    burrow|burrow-aesgcm|burrow-plaintext)
      local secret extra="" cextra=""
      secret="$("$BURROW" gensecret)"
      [ "$tool" = burrow-plaintext ] && extra="insecure_plaintext: true" && cextra="insecure_plaintext: true"
      [ "$tool" = burrow-aesgcm ] && cextra="cipher: aes-gcm"
      cat >"$dir/server.yaml" <<YAML
listen: 127.0.0.1:2333
$extra
limits:
  max_handshakes: 4096
  max_connections_per_client: 1000000
  max_pending: 100000
clients:
  bench:
    secret: $secret
services:
  echo: { client: bench, bind: 127.0.0.1:30001, target: 127.0.0.1:$ECHO_PORT }
  iperf: { client: bench, bind: 127.0.0.1:30002, target: 127.0.0.1:$IPERF_PORT }
YAML
      cat >"$dir/client.yaml" <<YAML
server: 127.0.0.1:2333
name: bench
secret: $secret
$cextra
allow: [127.0.0.1/32]
YAML
      chmod 600 "$dir"/*.yaml
      start "$BURROW" --quiet server -c "$dir/server.yaml"; TUNNEL_PIDS+=($!)
      sleep 0.3
      start "$BURROW" --quiet client -c "$dir/client.yaml"; TUNNEL_PIDS+=($!)
      EP=30001; IP=30002 ;;
    rathole|rathole-noise)
      local transport='type = "tcp"' ctransport='type = "tcp"'
      if [ "$tool" = rathole-noise ]; then
        local keys priv pub
        keys="$("$BIN/rathole" --genkey)"
        priv="$(echo "$keys" | sed -n '/Private Key/{n;p}')"
        pub="$(echo "$keys" | sed -n '/Public Key/{n;p}')"
        transport=$'type = "noise"\n[server.transport.noise]\nlocal_private_key = "'"$priv"'"'
        ctransport=$'type = "noise"\n[client.transport.noise]\nremote_public_key = "'"$pub"'"'
      fi
      cat >"$dir/server.toml" <<TOML
[server]
bind_addr = "127.0.0.1:2334"
[server.transport]
$transport
[server.services.echo]
token = "bench-token"
bind_addr = "127.0.0.1:30011"
[server.services.iperf]
token = "bench-token"
bind_addr = "127.0.0.1:30012"
TOML
      cat >"$dir/client.toml" <<TOML
[client]
remote_addr = "127.0.0.1:2334"
[client.transport]
$ctransport
[client.services.echo]
token = "bench-token"
local_addr = "127.0.0.1:$ECHO_PORT"
[client.services.iperf]
token = "bench-token"
local_addr = "127.0.0.1:$IPERF_PORT"
TOML
      start "$BIN/rathole" -s "$dir/server.toml"; TUNNEL_PIDS+=($!)
      sleep 0.3
      start "$BIN/rathole" -c "$dir/client.toml"; TUNNEL_PIDS+=($!)
      EP=30011; IP=30012 ;;
    frp|frp-plain|frp-nomux)
      local tls=true mux=true
      [ "$tool" = frp-plain ] && tls=false
      [ "$tool" = frp-nomux ] && mux=false
      cat >"$dir/frps.toml" <<TOML
bindAddr = "127.0.0.1"
bindPort = 2335
auth.token = "bench-token"
log.level = "error"
transport.tcpMux = $mux
TOML
      cat >"$dir/frpc.toml" <<TOML
serverAddr = "127.0.0.1"
serverPort = 2335
auth.token = "bench-token"
log.level = "error"
transport.tls.enable = $tls
transport.tcpMux = $mux
[[proxies]]
name = "echo"
type = "tcp"
localIP = "127.0.0.1"
localPort = $ECHO_PORT
remotePort = 30021
[[proxies]]
name = "iperf"
type = "tcp"
localIP = "127.0.0.1"
localPort = $IPERF_PORT
remotePort = 30022
TOML
      start "$BIN/frps" -c "$dir/frps.toml"; TUNNEL_PIDS+=($!)
      sleep 0.3
      start "$BIN/frpc" -c "$dir/frpc.toml"; TUNNEL_PIDS+=($!)
      EP=30021; IP=30022 ;;
    chisel)
      start "$BIN/chisel" server --host 127.0.0.1 --port 2336 --reverse --auth bench:bench-token; TUNNEL_PIDS+=($!)
      sleep 0.3
      start "$BIN/chisel" client --auth bench:bench-token http://127.0.0.1:2336 \
        "R:127.0.0.1:30031:127.0.0.1:$ECHO_PORT" "R:127.0.0.1:30032:127.0.0.1:$IPERF_PORT"; TUNNEL_PIDS+=($!)
      EP=30031; IP=30032 ;;
  esac
  wait_echo "$EP"
}

teardown() {
  for p in "${TUNNEL_PIDS[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null || true; done
  for p in "${TUNNEL_PIDS[@]:-}"; do [ -n "$p" ] && wait "$p" 2>/dev/null || true; done
  sleep 1
}

record() { echo "{\"tool\": \"$1\", \"test\": \"$2\", \"result\": $3}" >>"$RUN/results.jsonl"; }

start "$LOADGEN" echo --listen "127.0.0.1:$ECHO_PORT"
start iperf3 -s -B 127.0.0.1 -p "$IPERF_PORT"
sleep 0.5

for tool in $TOOLS; do
  echo "== $tool"
  setup "$tool"
  for streams in 1 4; do
    out="$(iperf3 -c 127.0.0.1 -p "$IP" -t "$IPERF_SECS" -P "$streams" -J)"
    bps="$(echo "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin)["end"]["sum_received"]["bits_per_second"])')"
    record "$tool" "throughput_p$streams" "{\"gbit_per_sec\": $(python3 -c "print(round($bps/1e9, 2))")}"
    echo "  throughput -P $streams: $(python3 -c "print(round($bps/1e9, 2))") Gbit/s"
  done
  r="$("$LOADGEN" latency --addr "127.0.0.1:$EP" --count 20000 --size 64)"; record "$tool" latency "$r"; echo "  latency: $r"
  iperf3 -c 127.0.0.1 -p "$IP" -t 8 -P 4 -b 500M >/dev/null 2>&1 &
  bulk=$!
  sleep 1
  r="$("$LOADGEN" latency --addr "127.0.0.1:$EP" --count 5000 --size 64)"; record "$tool" latency_under_load "$r"; echo "  latency under load: $r"
  wait "$bulk" 2>/dev/null || true
  r="$("$LOADGEN" connrate --addr "127.0.0.1:$EP" --secs "$CONNRATE_SECS" --concurrency 32)"; record "$tool" connrate "$r"; echo "  connrate: $r"
  if [ "$tool" != direct ]; then
    for n in $IDLE_COUNTS; do
      sleep 2
      before="$(rss_kib "${TUNNEL_PIDS[@]}")"
      "$LOADGEN" idle --addr "127.0.0.1:$EP" --count "$n" --hold-secs 600 >"$RUN/$tool/idle-$n.out" 2>&1 &
      gen=$!
      for _ in $(seq 1 600); do grep -q '"open"' "$RUN/$tool/idle-$n.out" 2>/dev/null && break; sleep 0.1; done
      sleep 3
      after="$(rss_kib "${TUNNEL_PIDS[@]}")"
      opened="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["open"])' "$RUN/$tool/idle-$n.out" 2>/dev/null || echo 0)"
      kill "$gen" 2>/dev/null || true; wait "$gen" 2>/dev/null || true
      record "$tool" "idle_$n" "{\"open\": $opened, \"rss_before_kib\": $before, \"rss_after_kib\": $after}"
      echo "  idle $n: open=$opened rss ${before}KiB -> ${after}KiB"
    done
  fi
  teardown
done

python3 "$ROOT/bench/report.py" "$RUN" >"$ROOT/docs/benchmark.md"
echo "results: $RUN/results.jsonl, report: docs/benchmark.md"
