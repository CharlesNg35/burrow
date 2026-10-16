# burrow

A fast, small, secure reverse tunnel. A client behind NAT connects out to a public server, and the **server's** config decides what gets exposed. Adding or removing a service is a server config edit: no client change, no restart.

```
visitor ──> server:7840 ══ Noise-encrypted TCP or WebSocket ══ client (behind NAT) ──> 127.0.0.1:3000
```

- TCP and UDP services, half-close, an adaptive pool of pre-opened data channels
- Noise `NNpsk0` with a separate 256-bit secret per client, and a client-side allow list (deny by default)
- Hot reload on file change or `SIGHUP`; invalid edits are rejected and the old config keeps running
- Optional WebSocket transport for servers that are only reachable over HTTP(S); off unless configured
- Clients never give up: jittered backoff, DNS re-resolved on every attempt, Happy Eyeballs dialing
- Every config error reported at once, with `file:line:column`, a caret and a fix
- Single static binary, about 3 MB; container image about 6 MB

## Quickstart

On the public server:

```sh
burrow init server --server myserver.example.com:7835   # writes server.yaml, prints the client config
burrow server -c server.yaml
```

On the machine behind NAT, save the printed client config as `client.yaml`, then:

```sh
burrow client -c client.yaml
```

The generated server listens for clients on TCP `7835` and exposes `0.0.0.0:7840` → the client's `127.0.0.1:3000`. Edit `services` in `server.yaml` and save; the server reloads by itself.

Other commands:

```sh
burrow validate -c server.yaml            # check a config without starting (exit 0 valid, 1 invalid, 2 usage)
burrow client-config home -c server.yaml --server myserver.example.com:7835   # client config for a client
burrow gensecret                          # new 256-bit secret
burrow schema server > server.schema.json # JSON Schema for editor completion
burrow completions bash > /etc/bash_completion.d/burrow
```

`burrow server` and `burrow client` run the same checks as `validate` before opening any socket, and the server runs them again on every reload.

```
error: unknown client "hom" referenced by service "web"
  --> server.yaml:9:13
   |
 9 |     client: hom
   |             ^^^ no client with this name
   = help: did you mean "home"? defined clients: home, office
```

## Configuration

YAML (`.yaml`, `.yml`) or JSON (`.json`), same schema. `${VAR}` expands environment variables (`$${` for a literal `${`). Unknown fields are errors.

### server.yaml

```yaml
listen: 0.0.0.0:7835
clients:
  home:
    secret: ${HOME_SECRET} # or a list of secrets while rotating
services:
  web:
    client: home
    bind: 0.0.0.0:7840 # a port (means 0.0.0.0:port) or ip:port
    target: 127.0.0.1:3000 # host:port as seen from the client
  dns:
    client: home
    protocol: udp
    bind: 5353
    target: 192.168.1.1:53
```

| key                                          | default                             | meaning                                                        |
| -------------------------------------------- | ----------------------------------- | -------------------------------------------------------------- |
| `listen`                                     | none                                | TCP address clients connect to (`listen`, `websocket` or both) |
| `websocket.listen` / `path`                  | none / `/burrow`                    | accept clients over WebSocket (see below)                      |
| `websocket.tls.cert` / `key`                 | none                                | serve `wss://` directly with these PEM files                   |
| `websocket.keepalive_secs`                   | `25`                                | keepalive on idle pooled connections, 5-30                     |
| `clients.<name>.secret`                      | required                            | base64 or hex 256-bit secret, or a list of up to 4             |
| `clients.<name>.max_connections`             | `limits.max_connections_per_client` | concurrent forwarded connections for this client               |
| `services.<name>.client` / `bind` / `target` | required                            | who reaches the target, public address, target address         |
| `services.<name>.protocol`                   | `tcp`                               | `tcp` or `udp`                                                 |
| `services.<name>.max_connections`            | unlimited                           | concurrent connections for this service                        |
| `insecure_plaintext`                         | `false`                             | do not encrypt forwarded TCP data (authentication still runs)  |
| `heartbeat.interval_secs`                    | `10`                                | ping interval; peers time out after 3 missed intervals         |
| `limits.handshake_timeout_secs`              | `5`                                 | time allowed to authenticate                                   |
| `limits.max_handshakes`                      | `256`                               | concurrent unauthenticated connections                         |
| `limits.max_connections_per_client`          | `8192`                              |                                                                |
| `limits.max_pending`                         | `512`                               | visitors waiting for a data channel, per client                |
| `pool.min_idle` / `pool.max_idle`            | `4` / `64`                          | pre-opened data channels per client (adaptive)                 |
| `tcp.nodelay` / `tcp.keepalive_secs`         | `true` / `30`                       | `keepalive_secs: 0` disables keepalive                         |
| `udp.session_timeout_secs`                   | `60`                                | idle UDP peer expiry                                           |

### client.yaml

```yaml
server: myserver.example.com:7835 # or wss://tunnel.example.com/burrow
name: home
secret: ${HOME_SECRET}
allow:
  - 127.0.0.0/8
  - 192.168.1.0/24:22 # CIDR, optionally :port or :low-high
```

| key                                             | default          | meaning                                                    |
| ----------------------------------------------- | ---------------- | ---------------------------------------------------------- |
| `server`, `name`, `secret`                      | required         | where to connect, this client's name, its secret           |
| `allow`                                         | empty (deny all) | targets the server may reach, checked after DNS resolution |
| `allow_all`                                     | `false`          | allow any target (not recommended)                         |
| `cipher`                                        | `chachapoly`     | or `aes-gcm` (about 1.8x tunnel throughput with AES-NI)    |
| `proxy`                                         | none             | `http://host:port` or `socks5://[user:pass@]host:port`     |
| `insecure_plaintext`                            | `false`          | must match the server                                      |
| `reconnect.initial_delay_ms` / `max_delay_secs` | `250` / `30`     | exponential backoff with jitter                            |
| `reconnect.give_up_after_secs`                  | `0` (never)      | exit after the server was unreachable this long            |
| `reconnect.on_rejected`                         | `retry`          | or `exit` when the server rejects the name or secret       |
| `headers`                                       | none             | extra HTTP headers for the WebSocket upgrade               |
| `ca`                                            | none             | extra CA certificate (PEM) to trust for `wss://`           |
| `connect_timeout_secs`                          | `10`             |                                                            |
| `tcp`, `udp`                                    | as on the server |                                                            |

## Servers reachable only through HTTP(S)

If the server sits behind nginx, Caddy, Cloudflare or a Kubernetes ingress and only HTTP(S) is open in the firewall, let clients connect over WebSocket. Nothing about it runs unless you turn it on:

```sh
burrow init server --ws --server tunnel.example.com   # no TCP control port; WebSocket on 127.0.0.1:7836/burrow
```

```yaml
websocket:
  listen: 127.0.0.1:7836
  path: /burrow
```

The client config simply uses the URL: `server: wss://tunnel.example.com/burrow`. Data channels, UDP services, the allow list and hot reload work the same way; the Noise encryption runs inside the WebSocket, so the proxy only ever sees ciphertext. Point the proxy at burrow:

```nginx
location /burrow {
    proxy_pass http://127.0.0.1:7836;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
    proxy_read_timeout 1h;
}
```

```caddy
tunnel.example.com {
    reverse_proxy /burrow 127.0.0.1:7836
}
```

With Cloudflare, proxy the hostname (orange cloud) to the origin that runs nginx or Caddy; WebSockets are on by default. Idle pooled connections send a keepalive every 25 s (`websocket.keepalive_secs`, 5-30), below the idle timeouts of Cloudflare (100 s) and nginx (60 s).

Without a proxy, burrow can serve `wss://` itself with your certificate files (for example from Let's Encrypt):

```sh
burrow init server --ws-tls-cert /etc/letsencrypt/live/tunnel.example.com/fullchain.pem \
  --ws-tls-key /etc/letsencrypt/live/tunnel.example.com/privkey.pem --server tunnel.example.com
```

This listens on `0.0.0.0:7837`; clients use `wss://tunnel.example.com:7837/burrow`. Send `SIGHUP` after renewing the certificate. A self-signed certificate works if the client trusts it with `ca: server-ca.pem`.

`listen` (TCP) and `websocket` can also be enabled together, for example while moving clients from one to the other.

## Reconnecting

A client keeps retrying until it is stopped: exponential backoff with jitter (250 ms up to 30 s), resolving the server name again on every attempt and trying all its addresses in parallel (Happy Eyeballs). If you move the server to another machine and update DNS, clients follow on their own. During a long outage the client logs the first failure and then one summary per minute. `reconnect.give_up_after_secs` and `reconnect.on_rejected: exit` make it stop instead, for setups where a supervisor should notice.

## Security notes

- **Secrets must be random.** Anyone who records a handshake can try guesses offline, so a secret must be 256 random bits. burrow rejects anything that is not exactly 32 bytes, and anything that looks non-random. Generate secrets with `burrow gensecret`, and never use a password.
- Each client has its own secret. To rotate, list the old and the new one (`secret: [old, new]`), switch the client, then remove the old one.
- Unknown client names and wrong secrets get exactly the same response (the connection is closed with no data) after the same work, so client names cannot be discovered.
- Forward secrecy comes from fresh X25519 keys per connection. The protocol and the exact point where each side is authenticated are described in [docs/protocol.md](docs/protocol.md).
- The client only connects to addresses in `allow`, checked after DNS resolution, so even a compromised server cannot use it to reach other hosts.
- Secrets never appear in logs or error messages (only their length or a short fingerprint). `validate` warns if a config file is readable by other users; use `chmod 600`.

## Docker

```sh
docker run --rm -v "$PWD/server.yaml:/etc/burrow/server.yaml:ro" ghcr.io/charlesng35/burrow validate -c /etc/burrow/server.yaml
docker run -d --name burrow -p 7835:7835 -p 7840:7840 --user "$(id -u):$(id -g)" \
  -v "$PWD/server.yaml:/etc/burrow/server.yaml:ro" ghcr.io/charlesng35/burrow
```

The image is distroless and static, runs as a non-root user, and reads its config from `/etc/burrow`. Run it with `--user` matching the owner of your `0600` config file. See [deploy/docker-compose.example.yml](deploy/docker-compose.example.yml) for a server and client pair, and [deploy/](deploy/) for systemd units (`systemctl reload burrow-server` sends `SIGHUP`).

## Benchmarks

One machine (i5-12600K, 10 vCPUs), everything on loopback, from `bench/run.sh`:

|                               | burrow (default)       | rathole 0.5 (noise) | frp 0.71 (default) | chisel 1.12 |
| ----------------------------- | ---------------------- | ------------------- | ------------------ | ----------- |
| throughput, 1 / 4 streams     | **12.8 / 35.1 Gbit/s** | 6.0 / 18.4          | 8.7 / 9.3          | 6.1 / 5.5   |
| new connections/s             | **13.8k**              | 6.3k                | 10.4k              | 11.6k       |
| p99 latency under load        | **498 µs**             | 660 µs              | 694 µs             | 1241 µs     |
| RSS with 10k idle connections | **172 MiB**            | 2788 MiB            | 777 MiB            | 701 MiB     |

With `cipher: aes-gcm` burrow reaches 22.6 / 52.3 Gbit/s. Unencrypted, burrow moves 2x rathole's plain TCP throughput, but opens 7% fewer connections per second, because it still authenticates every data channel with Noise.

Full table, machine details and exact commands: [docs/benchmark.md](docs/benchmark.md). Reproduce with `bench/run.sh`.

## Development

```sh
cargo test                                            # unit, integration, security, diagnostics, init
cargo test --release --test heavy -- --ignored       # 10k connections, soak, chaos, churn and leak checks
cd fuzz && cargo +nightly fuzz run config            # also: handshake, frames
```

Design notes: [docs/research.md](docs/research.md), [docs/decisions.md](docs/decisions.md).

## License

MIT
