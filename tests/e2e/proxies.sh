#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
WORK="$(mktemp -d)"
PIDS=()
CONTAINERS=()
cleanup() {
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null || true; done
  for c in "${CONTAINERS[@]}"; do docker rm -f "$c" >/dev/null 2>&1 || true; done
  rm -rf "$WORK"
}
trap cleanup EXIT

command -v docker >/dev/null || { echo "docker is required" >&2; exit 2; }
cargo build --release --manifest-path "$ROOT/Cargo.toml" --bin burrow --example loadgen -q
BURROW="$ROOT/target/release/burrow"
LOADGEN="$ROOT/target/release/examples/loadgen"

cd "$WORK"
openssl req -x509 -newkey rsa:2048 -nodes -keyout key.pem -out cert.pem -days 1 \
  -subj "/CN=localhost" -addext "subjectAltName=DNS:localhost" 2>/dev/null
chmod 644 key.pem cert.pem
SECRET="$("$BURROW" gensecret)"

cat >server.yaml <<YAML
websocket:
  listen: 127.0.0.1:17836
  path: /burrow
  keepalive_secs: 5
heartbeat:
  interval_secs: 5
clients:
  home:
    secret: $SECRET
services:
  echo:
    client: home
    bind: 127.0.0.1:17840
    target: 127.0.0.1:19000
YAML

cat >nginx.conf <<'NGINX'
map $http_upgrade $connection_upgrade { default upgrade; '' close; }
server {
    listen 127.0.0.1:18443 ssl;
    server_name localhost;
    ssl_certificate     /certs/cert.pem;
    ssl_certificate_key /certs/key.pem;
    location = /burrow {
        proxy_pass http://127.0.0.1:17836;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $connection_upgrade;
        proxy_set_header Host $host;
        proxy_read_timeout 8s;
        proxy_send_timeout 8s;
        proxy_buffering off;
    }
}
NGINX

cat >Caddyfile <<'CADDY'
{
    auto_https off
    admin off
}
https://localhost:18444 {
    tls /certs/cert.pem /certs/key.pem
    reverse_proxy /burrow 127.0.0.1:17836
}
CADDY

chmod 600 server.yaml
"$LOADGEN" echo --listen 127.0.0.1:19000 >/dev/null 2>&1 & PIDS+=($!)
"$BURROW" server -c server.yaml >server.log 2>&1 & PIDS+=($!)

docker run -d --name burrow-e2e-nginx --network host -v "$WORK/nginx.conf:/etc/nginx/conf.d/default.conf:ro" \
  -v "$WORK:/certs:ro" nginx:stable >/dev/null
CONTAINERS+=(burrow-e2e-nginx)
docker run -d --name burrow-e2e-caddy --network host -v "$WORK/Caddyfile:/etc/caddy/Caddyfile:ro" \
  -v "$WORK:/certs:ro" caddy:2 >/dev/null
CONTAINERS+=(burrow-e2e-caddy)
sleep 2

run_client() {
  local name="$1" url="$2"
  cat >"client-$name.yaml" <<YAML
server: $url
name: home
secret: $SECRET
ca: cert.pem
allow: [127.0.0.1/32:19000]
YAML
  chmod 600 "client-$name.yaml"
  "$BURROW" client -c "client-$name.yaml" >"client-$name.log" 2>&1 & PIDS+=($!)
  local pid=$!
  local ready=0
  for _ in $(seq 1 50); do
    if "$LOADGEN" latency --addr 127.0.0.1:17840 --count 10 >/dev/null 2>&1; then ready=1; break; fi
    sleep 0.2
  done
  if [ "$ready" != 1 ]; then
    echo "FAIL: $name: tunnel did not come up" >&2
    echo "--- client log" >&2; tail -20 "client-$name.log" >&2
    echo "--- server log" >&2; tail -20 server.log >&2
    echo "--- proxy log" >&2; docker logs "burrow-e2e-$name" 2>&1 | tail -20 >&2
    exit 1
  fi
  echo "== $name: $("$LOADGEN" latency --addr 127.0.0.1:17840 --count 2000)"
  echo "   idling 12s, past the proxy's 8s idle timeout"
  sleep 12
  "$LOADGEN" latency --addr 127.0.0.1:17840 --count 200 >/dev/null
  local reconnects
  reconnects="$(grep -c "connected to" "client-$name.log" || true)"
  if [ "$reconnects" != "1" ]; then
    echo "FAIL: $name reconnected ($reconnects connects); keepalive did not keep the channels open" >&2
    cat "client-$name.log" >&2
    exit 1
  fi
  echo "   ok: still on the first connection, traffic flows after the idle period"
  kill "$pid"
  wait "$pid" 2>/dev/null || true
  sleep 1
}

run_client nginx "wss://localhost:18443/burrow"
run_client caddy "wss://localhost:18444/burrow"
echo "all proxy end-to-end checks passed"
