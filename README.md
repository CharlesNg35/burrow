<p align="center">
  <img src="docs/logo.svg" width="120" alt="burrow logo">
</p>

<h1 align="center">burrow</h1>

<p align="center">
  <a href="https://github.com/CharlesNg35/burrow/actions/workflows/ci.yml"><img src="https://github.com/CharlesNg35/burrow/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/CharlesNg35/burrow/releases/latest"><img src="https://img.shields.io/github/v/release/CharlesNg35/burrow" alt="latest release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT license"></a>
  <img src="https://img.shields.io/badge/platforms-Linux%20%7C%20macOS%20%7C%20Windows-informational" alt="Linux, macOS, Windows">
  <a href="#docker"><img src="https://img.shields.io/badge/docker-ghcr.io%2Fcharlesng35%2Fburrow-2496ED?logo=docker&amp;logoColor=white" alt="Docker image"></a>
</p>

A fast, small, secure reverse tunnel. A client behind NAT connects out to a public server, and the **server's** config decides what gets exposed. Adding or removing a service is a server config edit: no client change, no restart.

```
user ──> server:7840 ══ Noise-encrypted TCP or WebSocket ══ client (behind NAT) ──> 127.0.0.1:3000
```

- TCP and UDP services, port ranges and Unix sockets
- Private services that only your own machines reach, with `burrow visit` or a SOCKS5 proxy
- Several clients per service, with failover or load balancing and health checks
- Per service: allowed source IPs, the real client address for your app, and speed limits
- Encrypted, with a separate secret per client; a client only reaches what it allows
- Config changes apply on save, without a restart
- Works behind nginx, Caddy or Cloudflare when only HTTP(S) is open
- Runs as a system service on Linux, macOS and Windows, with metrics, `burrow status` and `burrow update`

## Contents

- [Quickstart](#quickstart)
- [Configuration](#configuration)
- [Services](#services)
- [Private services and visitors](#private-services-and-visitors)
- [Servers reachable only through HTTP(S)](#servers-reachable-only-through-https)
- [Monitoring](#monitoring)
- [Administration (burrow admin)](#administration-burrow-admin)
- [Running as a service](#running-as-a-service)
- [Security notes](#security-notes)
- [Docker](#docker)
- [Benchmarks](#benchmarks)
- [Development](#development)
- [License](#license)

## Quickstart

[Download the latest release](https://github.com/CharlesNg35/burrow/releases/latest) for your system and unpack it (or use the [Docker](#docker) image). On the public server:

```sh
./burrow init server                                                  # writes server.yaml
./burrow server                                                       # runs it
./burrow client-config --server myserver.example.com -o client.yaml  # the config for your client
```

On the machine behind NAT, download burrow the same way, put `client.yaml` next to it, then:

```sh
./burrow client
```

The generated server listens for clients on TCP `7835` and exposes `0.0.0.0:7840` → the client's `127.0.0.1:3000`. Edit `services` in `server.yaml` and save; the server reloads by itself. To keep either side running across reboots, install it as a service: `sudo ./burrow service install -c server.yaml` on the server and `sudo ./burrow service install -c client.yaml` on the client (see [Running as a service](#running-as-a-service)).

Other commands:

```sh
./burrow validate -c server.yaml            # check a config without starting (exit 0 valid, 1 invalid, 2 usage)
./burrow client-config office --server myserver.example.com -o office.yaml   # config for a client added as office: under clients
./burrow gensecret                          # new 256-bit secret
./burrow schema server > server.schema.json # JSON Schema for editor completion
./burrow completions bash                   # shell completions: bash, zsh, fish, powershell, elvish
./burrow status                             # clients, services and traffic of the running servers (-f to follow)
./burrow edit                               # open a config in your editor; it is checked, saved and applied
./burrow update                             # install the latest release
```

## Configuration

YAML (`.yaml`, `.yml`) or JSON (`.json`), same schema. `${VAR}` expands environment variables (`$${` for a literal `${`). Unknown fields are errors, and `burrow validate` reports every problem with its line and a fix.

**server.yaml**

```yaml
# server.yaml
listen: 0.0.0.0:7835 # ip:port, or a port alone (means 0.0.0.0:port, all IPv4 addresses)
public_address: myserver.example.com # optional: how clients reach this server
clients:
  home:
    secret: ${HOME_SECRET} # or a list of secrets while rotating
services:
  web:
    client: home
    bind: 0.0.0.0:7840 # a port (means 0.0.0.0:port) or ip:port
    target: 127.0.0.1:3000 # host:port as seen from the client; a port alone means 127.0.0.1
  dns:
    client: home
    protocol: udp
    bind: 5353
    target: 192.168.1.1:53
```

<details>
<summary><b>All server.yaml keys</b></summary>

| key                                        | default                             | meaning                                                        |
| ------------------------------------------ | ----------------------------------- | -------------------------------------------------------------- |
| `listen`                                   | none                                | where the server waits for clients (and/or `websocket`)        |
| `public_address`                           | none                                | how clients reach it: host, host:port or wss:// URL            |
| `websocket.listen` / `path`                | none / `/burrow`                    | accept clients over WebSocket (see below)                      |
| `websocket.tls.cert` / `key`               | none                                | serve `wss://` directly with these PEM files                   |
| `websocket.keepalive_secs`                 | `25`                                | keepalive on idle connections, 5-30                            |
| `clients.<name>.secret`                    | required                            | base64 or hex 256-bit secret, or a list of up to 4             |
| `clients.<name>.max_connections`           | `limits.max_connections_per_client` | concurrent forwarded connections for this client               |
| `services.<name>.client` or `clients`      | required                            | the client that reaches the target, or several of them         |
| `services.<name>.target`                   | required                            | host:port, a port, a range like `30000-30010`, or `unix:/path` |
| `services.<name>.bind`                     | none (private)                      | public port, range or ip:port on the server                    |
| `services.<name>.protocol`                 | `tcp`                               | `tcp` or `udp`                                                 |
| `services.<name>.visitors`                 | none                                | visitors that may reach it with `burrow visit`                 |
| `services.<name>.mode`                     | `failover`                          | with several clients: `failover` or `balance`                  |
| `services.<name>.health_check`             | off                                 | `true`, or `{ interval_secs, timeout_secs, failures }`         |
| `services.<name>.allow_from` / `deny_from` | everyone                            | source IPs or CIDRs that may or may not connect                |
| `services.<name>.proxy_protocol`           | off                                 | `v1` or `v2`: send the real client address to the target       |
| `services.<name>.bandwidth`                | unlimited                           | per direction, for example `40mbit` or `5MB`                   |
| `services.<name>.max_connections`          | unlimited                           | concurrent connections for this TCP service                    |
| `visitors.<name>.secret`                   | required                            | like a client secret; see below                                |
| `visitors.<name>.reach`                    | none                                | `CLIENT: any`, or rules like `CLIENT: [192.168.1.0/24:22]`     |
| `visitors.<name>.max_connections`          | unlimited                           | concurrent connections for this visitor                        |
| `visitors.<name>.admin`                    | none                                | `read` or `write`: may use `burrow admin`, see below           |
| `visitors.<name>.admin_from`               | any address                         | source IPs or CIDRs that admin access is limited to            |
| `metrics.listen`                           | off                                 | address for `/metrics`, see [Monitoring](#monitoring)          |
| `insecure_plaintext`                       | `false`                             | do not encrypt forwarded TCP data (authentication still runs)  |
| `heartbeat.interval_secs`                  | `10`                                | ping interval; peers time out after 3 missed intervals         |
| `limits.handshake_timeout_secs`            | `5`                                 | time allowed to authenticate                                   |
| `limits.max_handshakes`                    | `256`                               | concurrent unauthenticated connections                         |
| `limits.max_connections_per_client`        | `8192`                              |                                                                |
| `limits.max_pending`                       | `512`                               | connections waiting for the client, per client                 |
| `pool.min_idle` / `pool.max_idle`          | `4` / `64`                          | connections kept ready per client                              |
| `tcp.nodelay` / `tcp.keepalive_secs`       | `true` / `30`                       | `keepalive_secs: 0` disables keepalive                         |
| `udp.session_timeout_secs`                 | `60`                                | idle UDP peer expiry                                           |

</details>

**client.yaml**

```yaml
# client.yaml
server: myserver.example.com:7835 # port 7835 when omitted; or wss://tunnel.example.com/burrow
name: home
secret: ${HOME_SECRET}
allow:
  - 127.0.0.0/8
  - 192.168.1.0/24:22 # CIDR, optionally :port or :low-high
  - 192.168.1.1/32:53 # the dns service above
  - unix:/run/postgresql/* # Unix sockets: one path, or every socket in a folder
```

The client connects only to targets in `allow`, checked after DNS resolution. `burrow client-config` writes this list from the server's services. Saving the file applies it, like on the server: open connections keep running unless their target is no longer allowed.

<details>
<summary><b>All client.yaml keys</b></summary>

| key                                             | default          | meaning                                                               |
| ----------------------------------------------- | ---------------- | --------------------------------------------------------------------- |
| `server`, `name`, `secret`                      | required         | where to connect, this client's name, its secret                      |
| `allow`                                         | empty (deny all) | targets the server may reach: CIDR with optional ports, or `unix:`    |
| `allow_all`                                     | `false`          | allow any target (not recommended)                                    |
| `cipher`                                        | `chachapoly`     | or `aes-gcm` (about 1.8x tunnel throughput with AES-NI)               |
| `proxy`                                         | none             | `http://host:port` or `socks5://[user:pass@]host:port`                |
| `insecure_plaintext`                            | `false`          | must match the server                                                 |
| `reconnect.initial_delay_ms` / `max_delay_secs` | `250` / `30`     | exponential backoff with jitter                                       |
| `reconnect.give_up_after_secs`                  | `0` (never)      | exit after this long without a working connection to the server       |
| `reconnect.on_rejected`                         | `retry`          | or `exit` when the server keeps rejecting the name or secret for 15 s |
| `headers`                                       | none             | extra HTTP headers for the WebSocket upgrade                          |
| `ca`                                            | none             | extra CA certificate (PEM) to trust for `wss://`                      |
| `connect_timeout_secs`                          | `10`             |                                                                       |
| `tcp`, `udp`                                    | as on the server |                                                                       |

A client keeps retrying until it is stopped, and follows a server that moved to a new address.

</details>

## Services

Everything about a service is set on the server; clients need no change, and a reload applies it.

**Port ranges.** One service can expose a range, where each public port leads to the same position in the target range. Visitors can forward ranges too.

```yaml
# server.yaml, under services:
game:
  client: home
  protocol: udp
  bind: 30000-30010
  target: 192.168.1.50:30000-30010
```

**Who may connect.** `allow_from` admits only these source addresses; `deny_from` blocks some. Blocked connections are closed on the server and never reach your network. A reload closes open connections that are no longer allowed.

```yaml
# server.yaml, under services:
ssh:
  client: home
  bind: 2222
  target: 22
  allow_from: [203.0.113.0/24, 2001:db8::/32]
```

**The real client address.** Behind a tunnel, your app sees every connection coming from the burrow client. With `proxy_protocol: v1` or `v2`, the server sends the real address first, in the PROXY protocol that nginx, Caddy, HAProxy, Traefik and Postfix understand. Turn it on only when the target expects it.

```nginx
# nginx.conf of the target app
server { listen 8443 ssl proxy_protocol; set_real_ip_from 127.0.0.1; real_ip_header proxy_protocol; }
```

**Several clients.** List more than one client to keep a service up when a machine goes away. `failover` (the default) uses the first connected client and moves to the next one while it is away; `balance` spreads new connections over all of them. When a client cannot reach the target or its `allow` list refuses it, the connection moves to the next client at once, and `burrow status` shows that client as `failing` or `refused`. With `health_check`, the server also checks through each client that its target answers, so a dead target is skipped before anyone connects. Open connections stay on their client.

```yaml
# server.yaml, under services:
web:
  clients: [home-1, home-2]
  mode: balance
  health_check: true # or { interval_secs: 10, timeout_secs: 3, failures: 2 }
  bind: 443
  target: 127.0.0.1:8443
```

**Speed limits.** Services are unlimited unless `bandwidth` is set. `bandwidth: 40mbit` (or `5MB`, bytes per second) caps each direction of a service, shared by all its connections. TCP slows down; UDP datagrams over the rate are dropped.

**Unix sockets.** `target: unix:/var/run/docker.sock` makes the client connect to a Unix socket. Its `allow` list must permit it, with `unix:/var/run/docker.sock` or `unix:/var/run/*`. A socket path that is itself a symbolic link is refused; allow the socket it points to. A client installed with `burrow service` cannot see sockets under `/tmp` or in home folders, and its account must be allowed to connect (`--run-as` picks another one).

## Private services and visitors

A service without `bind` opens no port on the server. Only the visitors it lists can reach it, from their own machine, through the server:

```
your PC: burrow visit 127.0.0.1:5432 ══ server ══ client (behind NAT) ──> 127.0.0.1:5432
```

In `server.yaml`, add a visitor with its own secret (from `./burrow gensecret`) and list it on the service:

```yaml
# server.yaml
visitors:
  laptop:
    secret: ${LAPTOP_SECRET}
services:
  db:
    client: home
    target: 5432 # no bind: private
    visitors: [laptop]
```

Then print the visitor's config on the server, and run it on your machine:

```sh
./burrow visitor-config laptop -o visitor.yaml   # on the server
./burrow visit                                   # on your machine: 127.0.0.1:5432 now leads to db
```

```yaml
# visitor.yaml
server: myserver.example.com
name: laptop
secret: ${LAPTOP_SECRET}
forwards:
  db: { service: db, local: 5432 } # a port alone means 127.0.0.1
```

`./burrow visit db --local 5433` runs one forward on another port, and `sudo ./burrow service install -c visitor.yaml` keeps the visitor running. Without access to the server, `./burrow init visitor` asks for its address, the visitor's name and secret, and writes the forwards this visitor may use; `./burrow init visitor --update` refreshes them later.

**Any target on a client.** With `reach`, a visitor picks the target itself, like `ssh -L`, without a service for each one:

```yaml
# server.yaml
visitors:
  laptop:
    secret: ${LAPTOP_SECRET}
    reach:
      home: any # any target that client home allows
      office: [192.168.150.0/25, 10.0.0.5/32:22] # only these, in allow-list syntax
```

```sh
./burrow visit --client home --target 22 --local 2222   # then: ssh -p 2222 localhost
```

The same in `visitor.yaml`: `ssh: { client: home, target: 22, local: 2222 }`.

**A SOCKS5 proxy into a client.** Instead of one forward per target, open one local SOCKS5 port and let each app name its target. `burrow visitor-config` writes one for each client in `reach`.

```sh
./burrow visit --client home --socks 1080
curl --socks5-hostname 127.0.0.1:1080 http://nas.local:5000   # or: ssh -o ProxyCommand='nc -X 5 -x 127.0.0.1:1080 %h %p' 192.168.1.20
```

In `visitor.yaml`: `home: { client: home, socks: true, local: 1080 }`. Ranges work as on the server, for a private TCP service with a port range: `admin: { service: admin, local: 40000-40010 }`.

- The client's `allow` list still has the final say: `reach` grants only what it permits, so keep it narrow (`validate` reminds you about `any`).
- With rules, a host name is checked after the client resolves it on its own network, so SOCKS apps that send names work too.
- Removing a visitor, its secret, or its access and reloading the server closes its open connections.
- Visitors are TCP only.

<details>
<summary><b>All visitor.yaml keys</b></summary>

| key                                                               | default  | meaning                                                         |
| ----------------------------------------------------------------- | -------- | --------------------------------------------------------------- |
| `server`, `name`, `secret`                                        | required | as in `client.yaml`, with the visitor's name and secret         |
| `forwards.<name>.service`                                         |          | a private service to reach                                      |
| `forwards.<name>.client` / `target`                               |          | or a client and a target on it (needs `reach`)                  |
| `forwards.<name>.socks`                                           | `false`  | with `client`: a SOCKS5 proxy, each connection names its target |
| `forwards.<name>.local`                                           | required | port, range or ip:port; a port alone means `127.0.0.1`          |
| `headers`, `ca`, `cipher`, `proxy`, `connect_timeout_secs`, `tcp` |          | as in `client.yaml`                                             |

</details>

## Servers reachable only through HTTP(S)

If the server sits behind nginx, Caddy, Cloudflare or a Kubernetes ingress and only HTTP(S) is open in the firewall, clients can connect over WebSocket. Nothing about it runs unless you turn it on:

```sh
./burrow init server --ws --server tunnel.example.com   # no TCP control port; WebSocket on 127.0.0.1:7836/burrow
```

```yaml
# server.yaml
public_address: tunnel.example.com
websocket:
  listen: 127.0.0.1:7836
  path: /burrow
```

The client config just uses the URL: `server: wss://tunnel.example.com/burrow`. Everything works exactly as over TCP, and the proxy only sees encrypted data. Idle connections send a keepalive every 25 s (`websocket.keepalive_secs`), below the idle timeouts of nginx and Cloudflare.

`listen` (TCP) and `websocket` can be enabled together, for example while moving clients from one to the other.

<details>
<summary><b>nginx</b></summary>

```nginx
# /etc/nginx/conf.d/burrow.conf
map $http_upgrade $connection_upgrade {
    default upgrade;
    ''      close;
}

server {
    listen 443 ssl;
    server_name tunnel.example.com;
    ssl_certificate     /etc/letsencrypt/live/tunnel.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/tunnel.example.com/privkey.pem;

    location = /burrow {
        proxy_pass http://127.0.0.1:7836;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $connection_upgrade;
        proxy_set_header Host $host;
        proxy_read_timeout 1h;
        proxy_send_timeout 1h;
        proxy_buffering off;
    }
}
```

Client: `server: wss://tunnel.example.com/burrow`

</details>

<details>
<summary><b>Caddy</b></summary>

Caddy gets the certificate itself and proxies WebSockets without extra settings:

```caddy
# Caddyfile
tunnel.example.com {
    reverse_proxy /burrow 127.0.0.1:7836
}
```

Client: `server: wss://tunnel.example.com/burrow`

</details>

<details>
<summary><b>Cloudflare (proxied DNS)</b></summary>

Point the DNS record at your server with the proxy enabled (orange cloud), and run nginx or Caddy on the origin as above. WebSockets are allowed on every plan. Use SSL mode "Full (strict)" so the hop between Cloudflare and your origin is also TLS.

To restrict the endpoint with [Cloudflare Access](https://developers.cloudflare.com/cloudflare-one/identity/service-tokens/), create a service token and send it from the client:

```yaml
# client.yaml (or visitor.yaml)
server: wss://tunnel.example.com/burrow
headers:
  CF-Access-Client-Id: ${CF_ACCESS_ID}
  CF-Access-Client-Secret: ${CF_ACCESS_SECRET}
```

</details>

<details>
<summary><b>Cloudflare Tunnel (no open inbound port at all)</b></summary>

`cloudflared` makes an outbound connection to Cloudflare, so the server needs no open ports for the control link:

```yaml
# /etc/cloudflared/config.yml
tunnel: <tunnel-id>
credentials-file: /etc/cloudflared/<tunnel-id>.json
ingress:
  - hostname: tunnel.example.com
    path: ^/burrow$
    service: http://127.0.0.1:7836
  - service: http_status:404
```

Client: `server: wss://tunnel.example.com/burrow`. The services you expose with `bind` are still plain TCP/UDP ports on the server, so open those (or bind them to a private interface) as needed.

</details>

<details>
<summary><b>Kubernetes (ingress-nginx)</b></summary>

```yaml
# ingress.yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: burrow
  annotations:
    nginx.ingress.kubernetes.io/proxy-read-timeout: "3600"
    nginx.ingress.kubernetes.io/proxy-send-timeout: "3600"
spec:
  ingressClassName: nginx
  tls:
    - hosts: [tunnel.example.com]
      secretName: tunnel-example-com-tls
  rules:
    - host: tunnel.example.com
      http:
        paths:
          - path: /burrow
            pathType: Exact
            backend:
              service:
                name: burrow
                port:
                  number: 7836
```

Bind `websocket.listen` to `0.0.0.0:7836` inside the pod so the Service can reach it.

</details>

<details>
<summary><b>No proxy: burrow serves wss:// itself</b></summary>

Give burrow a certificate and key (for example from Let's Encrypt):

```sh
./burrow init server \
  --ws-tls-cert /etc/letsencrypt/live/tunnel.example.com/fullchain.pem \
  --ws-tls-key  /etc/letsencrypt/live/tunnel.example.com/privkey.pem \
  --server tunnel.example.com
```

```yaml
# server.yaml
public_address: tunnel.example.com
websocket:
  listen: 0.0.0.0:7837
  path: /burrow
  tls:
    cert: /etc/letsencrypt/live/tunnel.example.com/fullchain.pem
    key: /etc/letsencrypt/live/tunnel.example.com/privkey.pem
```

Clients use `wss://tunnel.example.com:7837/burrow`. Relative file paths are resolved from the config file's directory. The key must be readable by the user burrow runs as. A renewed certificate is used without a restart; since a renewal writes a new key file, give the service account access again each time, for example with `--deploy-hook "setfacl -m u:burrow:r /etc/letsencrypt/live/tunnel.example.com/privkey.pem"` in certbot.

For a self-signed certificate, copy it to the client and trust it there with `ca: server-cert.pem`. `ca` accepts either a CA that signed the server certificate, or the server certificate itself, which is then pinned: only that exact certificate is accepted.

</details>

## Monitoring

`burrow status` shows every burrow server running on the machine, however it was started: by hand, by your own systemd unit, or with `burrow service install`. It needs no config and no metrics.

```sh
sudo burrow status               # every server on the machine
sudo burrow status burrow-server # only this installed service
burrow status -c server.yaml     # the server running this config
burrow status -f                 # refresh every second, with connections/s and bytes/s
burrow status -f --interval 5
burrow status --json             # the raw data, for scripts
```

A server installed as a service needs `sudo` (an administrator terminal on Windows). Clients and visitors have no status: use `burrow service status`.

<details>
<summary><b>Example output</b></summary>

```
$ sudo burrow status
burrow 0.1.0  ·  up 3 d 4 h  ·  clients on tcp 0.0.0.0:7835

CLIENTS  1/2 connected
╭────────┬─────────────┬────────────────┬────────────────┬──────┬──────╮
│ NAME   │ STATE       │ SINCE          │ FROM           │ OPEN │ IDLE │
├────────┼─────────────┼────────────────┼────────────────┼──────┼──────┤
│ home   │ ● connected │ 2 h 13 min     │ 81.2.3.4:51234 │    3 │    4 │
│ office │ ○ offline   │ left 5 min ago │                │    0 │    0 │
╰────────┴─────────────┴────────────────┴────────────────┴──────┴──────╯

SERVICES
╭──────┬───────┬─────────────┬────────────────┬─────────────────────────┬──────┬───────┬─────────┬────────┬────────╮
│ NAME │ PROTO │ ADDRESS     │ TARGET         │ CLIENTS                 │ OPEN │ TOTAL │ REFUSED │     IN │    OUT │
├──────┼───────┼─────────────┼────────────────┼─────────────────────────┼──────┼───────┼─────────┼────────┼────────┤
│ web  │ tcp   │ 0.0.0.0:443 │ 127.0.0.1:8443 │ home up, office offline │   12 │   153 │       1 │ 1.2 GB │ 8.4 GB │
╰──────┴───────┴─────────────┴────────────────┴─────────────────────────┴──────┴───────┴─────────┴────────┴────────╯
  web: balance, limit 5.0 MB/s, proxy protocol v2, source filter
```

</details>

**Prometheus.** Metrics are off unless you turn them on in `server.yaml`:

```yaml
# server.yaml
metrics:
  listen: 127.0.0.1:9100 # a port alone means 127.0.0.1
```

The server then serves Prometheus metrics at `/metrics`. Keep `listen` on `127.0.0.1` unless a firewall limits who can connect: anyone who reaches it sees names and traffic.

<details>
<summary><b>Metrics</b></summary>

| Metric                                      | Type           | Labels                 |                                                     |
| ------------------------------------------- | -------------- | ---------------------- | --------------------------------------------------- |
| `burrow_info`                               | gauge          | `version`              | always 1                                            |
| `burrow_uptime_seconds`                     | gauge          |                        |                                                     |
| `burrow_handshakes_refused_total`           | counter        |                        | connections refused before or during authentication |
| `burrow_udp_peers`                          | gauge          |                        | UDP peers with an open session                      |
| `burrow_client_connected`                   | gauge          | `client`               | 1 when connected                                    |
| `burrow_client_connections_open`            | gauge          | `client`               |                                                     |
| `burrow_service_connections_open`, `_total` | gauge, counter | `service`              |                                                     |
| `burrow_service_refused_total`              | counter        | `service`              | source address, limits, or no client                |
| `burrow_service_bytes_total`                | counter        | `service`, `direction` | `in` from users to the target, `out` back           |
| `burrow_service_client_up`                  | gauge          | `service`, `client`    | health check result, with `health_check` only       |
| `burrow_visitor_connections_open`, `_total` | gauge, counter | `visitor`              |                                                     |

</details>

## Administration (burrow admin)

`burrow admin` opens a local web UI for a running server: a live dashboard, and editing of clients, visitors, services and server settings with validation, a diff preview and comments kept. It runs on your machine with a visitor config, and the server exposes no new port: give that visitor admin rights in the server config.

```yaml
# server.yaml
visitors:
  ops-laptop:
    secret: ${OPS_SECRET}
    admin: write # read | write
    admin_from: [203.0.113.0/24] # optional: admin only from these addresses
```

```sh
burrow admin -c ops-laptop.yaml     # opens the browser on http://127.0.0.1:<random port>
```

- Use a dedicated visitor for admin; `validate` warns when it also reaches services.
- The UI listens on `127.0.0.1` only and needs a one-time token that is never on the command line. For a remote machine, use `ssh -L`.
- Secrets are generated by the server and shown once; existing secrets never leave the server. You cannot remove your own access.
- An installed server can write its config only when it has an admin visitor (or `service install --admin-writable`); otherwise the UI is read-only and says why.

## Running as a service

`burrow service` installs a server, client or visitor into the system service manager (systemd on Linux, launchd on macOS, the Service Control Manager on Windows), so it starts at boot and restarts after a failure. It needs administrator rights.

```sh
sudo ./burrow service install -c server.yaml   # or run it in the folder that holds server.yaml
burrow service status                          # state, version, account, the last log lines
sudo burrow service logs -f                    # follow the log
sudo burrow status -f                          # live clients and traffic
sudo burrow service restart                    # also: start, stop, reload, uninstall [--purge]
burrow service list
```

On Linux and macOS, `install` copies the binary to `/usr/local/bin` (when only root can change that folder), so after it `burrow` works from any folder, as in the commands above. On Windows, run them as `.\burrow.exe` from the download folder.

<details>
<summary><b>What <code>install</code> does</b></summary>

- Checks the config first, and checks that every port it listens on is free. If another program holds a port, it names it (`tcp port 7840 (service "web") is already in use by nginx (pid 812)`) and changes nothing.
- Copies the config to `/etc/burrow/` (`%ProgramData%\burrow\` on Windows), readable only by the service. Edit that copy, or use `burrow edit`: the service applies it on save, and `edit` refuses a change the service could not run after a restart, such as a port below 1024 it was not installed for. `status` tells you when the file you installed from has changed since. A server with an admin visitor (or installed with `--admin-writable`) gets its own folder, `/etc/burrow/<name>/server.yaml`, that only its account may change, so `burrow admin` can edit it; every other service keeps a config it cannot change.
- Copies the binary to `/usr/local/bin/burrow` (`%ProgramFiles%\burrow\` on Windows), unless it already lives in a root-owned folder such as one managed by a package manager. If other users can write `/usr/local/bin`, the service gets its own copy in a root-only folder. A service never runs a binary from a user's home folder.
- Runs the service as a dedicated account without login or home: `burrow` on Linux, `_burrow` on macOS, the virtual account `NT SERVICE\burrow-<name>` on Windows. `--run-as` picks another one. Ports below 1024 get only `CAP_NET_BIND_SERVICE` on Linux.
- Writes logs to `/var/log/burrow/<name>/<name>.log` (`%ProgramData%\burrow\logs\<name>.log` on Windows), rotated at 10 MB with 5 old files kept, so a log never fills the disk.

The service name comes from the config file name (`server.yaml` → `burrow-server`) or `--name`. You can install several servers and clients side by side. The same tunnel cannot run twice: a second copy of a server config, or a second client with the same name for the same server, is refused while the first one runs. A service itself is never kept from starting by a copy started by hand: it starts with a warning, and the server then refuses the duplicate client or the duplicate ports.

Variables used as `${VAR}` in the config are saved for the service at install time, readable by it only. `sudo` drops environment variables, so keep them with `sudo --preserve-env=HOME_SECRET ./burrow service install`; the install says so if one is missing.

</details>

**Updating.** `sudo burrow update` downloads the latest release for your system, checks it against the release's `SHA256SUMS` and runs it to confirm its version, then replaces the installed binary and restarts every service that uses it. If a service does not come up again, the old binary is put back. Without `sudo` it only replaces the binary you ran. `burrow update --check` only reports whether a newer release exists, and `--to 0.1.0` installs a given release.

<details>
<summary><b>More about updating</b></summary>

- The same version is left alone unless you add `--force`.
- Downloads go through `HTTPS_PROXY` (or `ALL_PROXY`) when set, and `NO_PROXY` is respected. `BURROW_UPDATE_URL` points to a mirror with the same layout as the GitHub releases; it must use https:// unless it is on the same machine.
- `sudo ./burrow service update` does the same with a binary you downloaded yourself. A binary installed by a package manager is upgraded there instead, and in a container you pull the new image.

</details>

<details>
<summary><b>Linux (systemd)</b></summary>

The unit is written to `/etc/systemd/system/burrow-<name>.service` and is sandboxed: read-only system, no home folders, no new privileges, and no capabilities unless a port below 1024 needs one. A config error stops it with exit code 3 instead of restarting it in a loop. `burrow service install --print` shows the unit without installing anything, if you prefer to manage it yourself.

</details>

<details>
<summary><b>macOS (launchd)</b></summary>

The job is written to `/Library/LaunchDaemons/com.github.charlesng35.burrow-<name>.plist`. launchd restarts it after a failure, at most once every 10 seconds.

</details>

<details>
<summary><b>Windows</b></summary>

Run the commands from a terminal opened with "Run as administrator". The service starts automatically after the network stack, and Windows restarts it 5 seconds after a failure (30 seconds from the third failure in a day). Set variables used by the config in the same terminal before installing, for example `$env:HOME_SECRET = "..."`. `reload` restarts the service on Windows; every role also applies its config on save.

</details>

## Security notes

- **Secrets must be random.** Anyone who records a handshake can try guesses offline, so a secret must be 256 random bits. burrow rejects anything that is not exactly 32 bytes, and anything that looks non-random. Generate secrets with `burrow gensecret`, and never use a password.
- Each client and visitor has its own secret. To rotate, list the old and the new one (`secret: [old, new]`), switch the client, then remove the old one.
- Unknown client or visitor names and wrong secrets get exactly the same response (one refusal byte, then the connection closes) after the same work, so names cannot be discovered.
- Forward secrecy comes from fresh X25519 keys per connection. The protocol and the exact point where each side is authenticated are described in [docs/protocol.md](docs/protocol.md).
- The client only connects to addresses in `allow`, checked after DNS resolution, so even a compromised server, or a visitor with `reach`, cannot use it to reach other hosts.
- A visitor reaches only the services that list it, and clients only with `reach`. A private service has no port on the server at all.
- `allow_from` and `deny_from` are checked on the server before anything enters the tunnel, also for visitors of private services. A visitor that comes through a proxy in front of `websocket.listen` has the proxy's address, and `validate` warns where a service relies on it. Unix socket targets need a `unix:` entry in the client's `allow` list.
- Secrets never appear in logs or error messages (only their length or a short fingerprint). `validate` warns if a config file is readable by every user; use `chmod 600`.
- Removing a client or visitor, or the secret it connected with, and reloading also closes its open connections.
- The lock that stops the same tunnel from running twice lives in a folder every local user can write to, so it also catches copies started by hand. A lock held by another user's process is ignored with a warning, so nobody can keep a service from starting that way.

## Docker

```sh
docker run --rm --user "$(id -u):$(id -g)" -v "$PWD/burrow:/etc/burrow:ro" ghcr.io/charlesng35/burrow validate -c /etc/burrow/server.yaml
docker run -d --name burrow -p 7835:7835 -p 7840:7840 --user "$(id -u):$(id -g)" \
  -v "$PWD/burrow:/etc/burrow:ro" ghcr.io/charlesng35/burrow
```

The image is distroless and static, runs as a non-root user, and reads its config from `/etc/burrow`. Run it with `--user` matching the owner of your `0600` config file. Mount the folder that holds the config, not the file itself: an editor that saves by writing a new file would leave a file mount on the old one, and the change would never apply.

<details>
<summary><b>Docker Compose: a server and a client</b></summary>

```yaml
# compose.yaml
services:
  burrow-server:
    image: ghcr.io/charlesng35/burrow:latest
    command: ["server", "-c", "/etc/burrow/server.yaml"]
    user: "${UID:-1000}:${GID:-1000}"
    volumes:
      - ./server:/etc/burrow:ro
    ports:
      - "7835:7835"
      - "7840:7840"
    restart: unless-stopped

  burrow-client:
    image: ghcr.io/charlesng35/burrow:latest
    command: ["client", "-c", "/etc/burrow/client.yaml"]
    user: "${UID:-1000}:${GID:-1000}"
    environment:
      HOME_SECRET: ${HOME_SECRET:-}
    volumes:
      - ./client:/etc/burrow:ro
    network_mode: host
    restart: unless-stopped
```

</details>

## Benchmarks

One machine (i5-12600K, 10 vCPUs), everything on loopback, from `bench/run.sh`:

|                               | burrow (default)       | rathole 0.5 (noise) | frp 0.71 (default) | chisel 1.12 |
| ----------------------------- | ---------------------- | ------------------- | ------------------ | ----------- |
| throughput, 1 / 4 streams     | **12.6 / 33.4 Gbit/s** | 5.4 / 18.1          | 8.5 / 8.8          | 6.0 / 5.6   |
| new connections/s             | **13.5k**              | 5.9k                | 10.0k              | 11.8k       |
| p99 latency under load        | **549 µs**             | 769 µs              | 678 µs             | 1057 µs     |
| RSS with 10k idle connections | **220 MiB**            | 2784 MiB            | 796 MiB            | 719 MiB     |

With `cipher: aes-gcm` burrow reaches 22.3 / 49.9 Gbit/s. Over WebSocket it does 11.8 / 31.1 Gbit/s with 391 MiB for 10k idle connections (chisel, also WebSocket: 6.0 / 5.6 Gbit/s, 719 MiB). Unencrypted, burrow moves 2.1x rathole's plain TCP throughput with one stream (1.3x with four) and opens 7% fewer connections per second (14.2k vs 15.3k).

Full table, machine details and exact commands: [docs/benchmark.md](docs/benchmark.md). Reproduce with `bench/run.sh`.

## Development

```sh
cargo test                                            # unit, integration, security, diagnostics, init
cargo test --release --test heavy -- --ignored --test-threads=1   # 10k connections, soak, chaos, churn and leak checks
cd fuzz && cargo +nightly fuzz run config            # also: handshake, frames
tests/e2e/proxies.sh                                  # wss:// through real nginx and Caddy containers (needs docker)
BURROW_TEST_TRANSPORT=ws cargo test --test integration   # run the integration suite over WebSocket
```

Design notes: [docs/research.md](docs/research.md), [docs/decisions.md](docs/decisions.md).

## License

MIT
