# burrow

A fast, small, secure reverse tunnel. A client behind NAT connects out to a public server, and the **server's** config decides what gets exposed. Adding or removing a service is a server config edit: no client change, no restart.

```
user ──> server:7840 ══ Noise-encrypted TCP or WebSocket ══ client (behind NAT) ──> 127.0.0.1:3000
```

- TCP and UDP services, half-close, an adaptive pool of pre-opened data channels
- Private services: reach a service from your own machine with `burrow visit`, without a public port
- Noise `NNpsk0` with a separate 256-bit secret per client, and a client-side allow list (deny by default)
- Hot reload on file change or `SIGHUP`; invalid edits are rejected and the old config keeps running
- Optional WebSocket transport for servers that are only reachable over HTTP(S); off unless configured
- Clients never give up: jittered backoff, DNS re-resolved on every attempt, Happy Eyeballs dialing
- Every config error reported at once, with `file:line:column`, a caret and a fix
- Single static binary, about 3 MB; container image about 6 MB

## Contents

- [Install](#install)
- [Quickstart](#quickstart)
- [Configuration](#configuration)
  - [server.yaml](#serveryaml)
  - [client.yaml](#clientyaml)
- [Private services and visitors](#private-services-and-visitors)
- [Servers reachable only through HTTP(S)](#servers-reachable-only-through-https)
- [Reconnecting](#reconnecting)
- [Running as a service](#running-as-a-service)
- [Security notes](#security-notes)
- [Docker](#docker)
- [Benchmarks](#benchmarks)
- [Development](#development)
- [License](#license)

## Install

Download the archive for your system from the [latest release](https://github.com/CharlesNg35/burrow/releases/latest) (checksums in `SHA256SUMS`), unpack it, and run `./burrow` from that folder (`.\burrow.exe` on Windows). Or use the [Docker](#docker) image.

## Quickstart

On the public server:

```sh
./burrow init server                                                  # writes server.yaml
./burrow server                                                       # runs it
./burrow client-config --server myserver.example.com -o client.yaml  # the config for your client
```

On the machine behind NAT, [download burrow](#install) the same way, put `client.yaml` next to it, then:

```sh
./burrow client
```

The generated server listens for clients on TCP `7835` and exposes `0.0.0.0:7840` → the client's `127.0.0.1:3000`. Edit `services` in `server.yaml` and save; the server reloads by itself. To keep either side running across reboots, install it as a service: `sudo ./burrow service install -c server.yaml` on the server and `sudo ./burrow service install -c client.yaml` on the client (see [Running as a service](#running-as-a-service)).

Other commands:

```sh
./burrow validate -c server.yaml            # check a config without starting (exit 0 valid, 1 invalid, 2 usage)
./burrow client-config office --server myserver.example.com -o office.yaml   # config for another client
./burrow gensecret                          # new 256-bit secret
./burrow schema server > server.schema.json # JSON Schema for editor completion
./burrow completions bash                   # shell completions: bash, zsh, fish, powershell, elvish
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

| key                                  | default                             | meaning                                                       |
| ------------------------------------ | ----------------------------------- | ------------------------------------------------------------- |
| `listen`                             | none                                | where the server waits for clients (and/or `websocket`)       |
| `public_address`                     | none                                | how clients reach it: host, host:port or wss:// URL           |
| `websocket.listen` / `path`          | none / `/burrow`                    | accept clients over WebSocket (see below)                     |
| `websocket.tls.cert` / `key`         | none                                | serve `wss://` directly with these PEM files                  |
| `websocket.keepalive_secs`           | `25`                                | keepalive on idle pooled connections, 5-30                    |
| `clients.<name>.secret`              | required                            | base64 or hex 256-bit secret, or a list of up to 4            |
| `clients.<name>.max_connections`     | `limits.max_connections_per_client` | concurrent forwarded connections for this client              |
| `services.<name>.client` / `target`  | required                            | who reaches the target, and the target address                |
| `services.<name>.bind`               | none (private)                      | public address on the server                                  |
| `services.<name>.visitors`           | none                                | visitors that may reach it with `burrow visit`                |
| `services.<name>.protocol`           | `tcp`                               | `tcp` or `udp`                                                |
| `services.<name>.max_connections`    | unlimited                           | concurrent connections for this service                       |
| `visitors.<name>.secret`             | required                            | like a client secret; see below                               |
| `visitors.<name>.reach`              | none                                | `CLIENT: any`: may pick any target that client allows         |
| `visitors.<name>.max_connections`    | unlimited                           | concurrent connections for this visitor                       |
| `insecure_plaintext`                 | `false`                             | do not encrypt forwarded TCP data (authentication still runs) |
| `heartbeat.interval_secs`            | `10`                                | ping interval; peers time out after 3 missed intervals        |
| `limits.handshake_timeout_secs`      | `5`                                 | time allowed to authenticate                                  |
| `limits.max_handshakes`              | `256`                               | concurrent unauthenticated connections                        |
| `limits.max_connections_per_client`  | `8192`                              |                                                               |
| `limits.max_pending`                 | `512`                               | connections waiting for a data channel, per client            |
| `pool.min_idle` / `pool.max_idle`    | `4` / `64`                          | pre-opened data channels per client (adaptive)                |
| `tcp.nodelay` / `tcp.keepalive_secs` | `true` / `30`                       | `keepalive_secs: 0` disables keepalive                        |
| `udp.session_timeout_secs`           | `60`                                | idle UDP peer expiry                                          |

### client.yaml

```yaml
server: myserver.example.com:7835 # port 7835 when omitted; or wss://tunnel.example.com/burrow
name: home
secret: ${HOME_SECRET}
allow:
  - 127.0.0.0/8
  - 192.168.1.0/24:22 # CIDR, optionally :port or :low-high
  - 192.168.1.1/32:53 # the dns service above
```

| key                                             | default          | meaning                                                               |
| ----------------------------------------------- | ---------------- | --------------------------------------------------------------------- |
| `server`, `name`, `secret`                      | required         | where to connect, this client's name, its secret                      |
| `allow`                                         | empty (deny all) | targets the server may reach, checked after DNS resolution            |
| `allow_all`                                     | `false`          | allow any target (not recommended)                                    |
| `cipher`                                        | `chachapoly`     | or `aes-gcm` (about 1.8x tunnel throughput with AES-NI)               |
| `proxy`                                         | none             | `http://host:port` or `socks5://[user:pass@]host:port`                |
| `insecure_plaintext`                            | `false`          | must match the server                                                 |
| `reconnect.initial_delay_ms` / `max_delay_secs` | `250` / `30`     | exponential backoff with jitter                                       |
| `reconnect.give_up_after_secs`                  | `0` (never)      | exit after the server was unreachable this long                       |
| `reconnect.on_rejected`                         | `retry`          | or `exit` when the server keeps rejecting the name or secret for 15 s |
| `headers`                                       | none             | extra HTTP headers for the WebSocket upgrade                          |
| `ca`                                            | none             | extra CA certificate (PEM) to trust for `wss://`                      |
| `connect_timeout_secs`                          | `10`             |                                                                       |
| `tcp`, `udp`                                    | as on the server |                                                                       |

## Private services and visitors

A service without `bind` opens no port on the server. Only the visitors it lists can reach it, from their own machine, through the server:

```
your PC: burrow visit 127.0.0.1:5432 ══ server ══ client (behind NAT) ──> 127.0.0.1:5432
```

In `server.yaml`, add a visitor with its own secret (from `./burrow gensecret`) and list it on the service:

```yaml
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
server: myserver.example.com
name: laptop
secret: ${LAPTOP_SECRET}
forwards:
  db: { service: db, local: 5432 } # a port alone means 127.0.0.1
```

`./burrow visit db --local 5433` runs one forward on another port, and `sudo ./burrow service install -c visitor.yaml` keeps the visitor running. `./burrow init visitor` writes a `visitor.yaml` from a name and a secret if you do not have access to the server.

**Any target on a client.** With `reach`, a visitor picks the target itself, like `ssh -L`, without a service for each one:

```yaml
visitors:
  laptop:
    secret: ${LAPTOP_SECRET}
    reach:
      home: any # any target that client home allows
```

```sh
./burrow visit --client home --target 22 --local 2222   # then: ssh -p 2222 localhost
```

The same in `visitor.yaml`: `ssh: { client: home, target: 22, local: 2222 }`.

- The client's `allow` list still has the final say: `reach` grants only what it permits, so keep it narrow (`validate` reminds you).
- Removing a visitor, its secret, or its access and reloading the server closes its open connections.
- Each local connection opens its own encrypted connection to the server. Visitors are TCP only, and need a server running burrow 0.2.0 or later.

| key                                                               | default  | meaning                                                                        |
| ----------------------------------------------------------------- | -------- | ------------------------------------------------------------------------------ |
| `server`, `name`, `secret`                                        | required | as in `client.yaml`, with the visitor's name and secret                        |
| `forwards.<name>.service`                                         |          | a private service to reach                                                     |
| `forwards.<name>.client` / `target`                               |          | or a client and a target on it (needs `reach`)                                 |
| `forwards.<name>.local`                                           | required | where to listen: a port alone means `127.0.0.1`; `validate` warns on `0.0.0.0` |
| `headers`, `ca`, `cipher`, `proxy`, `connect_timeout_secs`, `tcp` |          | as in `client.yaml`                                                            |

## Servers reachable only through HTTP(S)

If the server sits behind nginx, Caddy, Cloudflare or a Kubernetes ingress and only HTTP(S) is open in the firewall, clients can connect over WebSocket. Nothing about it runs unless you turn it on:

```sh
./burrow init server --ws --server tunnel.example.com   # no TCP control port; WebSocket on 127.0.0.1:7836/burrow
```

```yaml
public_address: tunnel.example.com
websocket:
  listen: 127.0.0.1:7836
  path: /burrow
```

The client config just uses the URL: `server: wss://tunnel.example.com/burrow`. Services (TCP and UDP), the allow list and hot reload work exactly as over TCP. Noise encryption runs inside the WebSocket, so the proxy only ever sees ciphertext. Idle pooled connections send a keepalive every 25 s (`websocket.keepalive_secs`, 5-30), below the idle timeouts of nginx (60 s) and Cloudflare (100 s).

`listen` (TCP) and `websocket` can be enabled together, for example while moving clients from one to the other.

<details>
<summary><b>nginx</b></summary>

```nginx
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
public_address: tunnel.example.com
websocket:
  listen: 0.0.0.0:7837
  path: /burrow
  tls:
    cert: /etc/letsencrypt/live/tunnel.example.com/fullchain.pem
    key: /etc/letsencrypt/live/tunnel.example.com/privkey.pem
```

Clients use `wss://tunnel.example.com:7837/burrow`. Relative file paths are resolved from the config file's directory. The key must be readable by the user burrow runs as. Reload after renewing the certificate:

```sh
certbot renew --deploy-hook "systemctl reload burrow-server"
```

For a self-signed certificate, copy it to the client and trust it there with `ca: server-cert.pem`. `ca` accepts either a CA that signed the server certificate, or the server certificate itself, which is then pinned: only that exact certificate is accepted.

</details>

## Reconnecting

A client keeps retrying until it is stopped: exponential backoff with jitter (250 ms up to 30 s), resolving the server name again on every attempt and trying all its addresses in parallel (Happy Eyeballs). If you move the server to another machine and update DNS, clients follow on their own. During a long outage the client logs the first failure and then one summary per minute. `reconnect.give_up_after_secs` and `reconnect.on_rejected: exit` make it stop instead, for setups where a supervisor should notice.

## Running as a service

`burrow service` installs a server, client or visitor into the system service manager (systemd on Linux, launchd on macOS, the Service Control Manager on Windows), so it starts at boot and restarts after a failure. It needs administrator rights.

```sh
sudo ./burrow service install -c server.yaml   # or run it in the folder that holds server.yaml
burrow service status                          # state, version, account, the last log lines
sudo burrow service logs -f                    # follow the log
sudo burrow service restart                    # also: start, stop, reload, uninstall [--purge]
burrow service list
```

On Linux and macOS, `install` copies the binary to `/usr/local/bin`, so after it `burrow` works from any folder, as in the commands above. On Windows, run them as `.\burrow.exe` from the download folder.

What `install` does:

- Checks the config first, and checks that every port it listens on is free. If another program holds a port, it names it (`tcp port 7840 (service "web") is already in use by nginx (pid 812)`) and changes nothing.
- Copies the config to `/etc/burrow/` (`%ProgramData%\burrow\` on Windows), readable only by the service. Edit that copy: a server reloads it on save, a client or visitor applies it on `burrow service restart`. `status` tells you when the file you installed from has changed since.
- Copies the binary to `/usr/local/bin/burrow` (`%ProgramFiles%\burrow\` on Windows), unless it already lives in a root-owned folder such as one managed by a package manager. A service never runs a binary from a user's home folder.
- Runs the service as a dedicated account without login or home: `burrow` on Linux, `_burrow` on macOS, the virtual account `NT SERVICE\burrow-<name>` on Windows. `--run-as` picks another one. Ports below 1024 get only `CAP_NET_BIND_SERVICE` on Linux.
- Writes logs to `/var/log/burrow/<name>/<name>.log` (`%ProgramData%\burrow\logs\<name>.log` on Windows), rotated at 10 MB with 5 old files kept, so a log never fills the disk.

The service name comes from the config file name (`server.yaml` → `burrow-server`) or `--name`. You can install several servers and clients side by side. The same tunnel cannot run twice: a second copy of a server config, or a second client with the same name for the same server, is refused while the first one runs, whether it was started as a service or by hand.

Variables used as `${VAR}` in the config are saved for the service at install time, readable by it only. `sudo` drops environment variables, so keep them with `sudo --preserve-env=HOME_SECRET ./burrow service install`; the install says so if one is missing.

To update, run `sudo burrow update`. It downloads the latest release for your system from GitHub, checks it against the release's `SHA256SUMS`, makes sure it runs and reports the expected version, then replaces the installed binary, restarts every service that uses it, and puts the old binary back if a service does not come up again. Nothing changes when any of these checks fails. Without `sudo` it only replaces the binary you ran and tells you how to update the services.

- `burrow update --check` only says whether a newer release exists.
- `burrow update --to 0.2.0` installs that release, older ones included. The same version is left alone unless you add `--force`.
- It goes through `HTTPS_PROXY` (or `ALL_PROXY`) when set, and `NO_PROXY` is respected. `BURROW_UPDATE_URL` points it to a mirror with the same layout as the GitHub releases.
- `sudo ./burrow service update` does the same with a binary you downloaded yourself. A binary installed by a package manager is upgraded there instead, and in a container you pull the new image.

<details>
<summary><b>Linux (systemd)</b></summary>

The unit is written to `/etc/systemd/system/burrow-<name>.service` and is sandboxed (`systemd-analyze security` rates it 1.1, "OK"): read-only system, no home folders, private `/tmp` and devices, no new privileges, a system call filter, and no capabilities unless a port below 1024 needs one. A config error stops it with exit code 3 instead of restarting it in a loop. `burrow service install --print` shows the unit without installing anything, if you prefer to manage it yourself.

</details>

<details>
<summary><b>macOS (launchd)</b></summary>

The job is written to `/Library/LaunchDaemons/com.github.charlesng35.burrow-<name>.plist` and is managed with `launchctl bootstrap`, `bootout` and `kickstart`. launchd restarts it after a failure, at most once every 10 seconds.

</details>

<details>
<summary><b>Windows</b></summary>

Run the commands from a terminal opened with "Run as administrator". The service starts automatically after the network stack, and Windows restarts it 5 seconds after a failure. Set variables used by the config in the same terminal before installing, for example `$env:HOME_SECRET = "..."`. `reload` restarts the service on Windows; a server also reloads its config on save.

</details>

## Security notes

- **Secrets must be random.** Anyone who records a handshake can try guesses offline, so a secret must be 256 random bits. burrow rejects anything that is not exactly 32 bytes, and anything that looks non-random. Generate secrets with `burrow gensecret`, and never use a password.
- Each client and visitor has its own secret. To rotate, list the old and the new one (`secret: [old, new]`), switch the client, then remove the old one.
- Unknown client or visitor names and wrong secrets get exactly the same response (the connection is closed with no data) after the same work, so names cannot be discovered.
- Forward secrecy comes from fresh X25519 keys per connection. The protocol and the exact point where each side is authenticated are described in [docs/protocol.md](docs/protocol.md).
- The client only connects to addresses in `allow`, checked after DNS resolution, so even a compromised server, or a visitor with `reach`, cannot use it to reach other hosts.
- A visitor reaches only the services that list it, and clients only with `reach`. A private service has no port on the server at all.
- Secrets never appear in logs or error messages (only their length or a short fingerprint). `validate` warns if a config file is readable by other users; use `chmod 600`.
- Removing a client or visitor, or the secret it connected with, and reloading also closes its open connections.
- The lock that stops the same tunnel from running twice lives in a folder every local user can write to, so it also catches copies started by hand. On a machine shared with untrusted users, one of them could hold a service's lock and keep it from starting.

## Docker

```sh
docker run --rm --user "$(id -u):$(id -g)" -v "$PWD/server.yaml:/etc/burrow/server.yaml:ro" ghcr.io/charlesng35/burrow validate -c /etc/burrow/server.yaml
docker run -d --name burrow -p 7835:7835 -p 7840:7840 --user "$(id -u):$(id -g)" \
  -v "$PWD/server.yaml:/etc/burrow/server.yaml:ro" ghcr.io/charlesng35/burrow
```

The image is distroless and static, runs as a non-root user, and reads its config from `/etc/burrow`. Run it with `--user` matching the owner of your `0600` config file.

<details>
<summary><b>Docker Compose: a server and a client</b></summary>

```yaml
services:
  burrow-server:
    image: ghcr.io/charlesng35/burrow:latest
    command: ["server", "-c", "/etc/burrow/server.yaml"]
    user: "${UID:-1000}:${GID:-1000}"
    volumes:
      - ./server.yaml:/etc/burrow/server.yaml:ro
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
      - ./client.yaml:/etc/burrow/client.yaml:ro
    network_mode: host
    restart: unless-stopped
```

</details>

## Benchmarks

One machine (i5-12600K, 10 vCPUs), everything on loopback, from `bench/run.sh`:

|                               | burrow (default)       | rathole 0.5 (noise) | frp 0.71 (default) | chisel 1.12 |
| ----------------------------- | ---------------------- | ------------------- | ------------------ | ----------- |
| throughput, 1 / 4 streams     | **12.7 / 34.7 Gbit/s** | 6.5 / 18.2          | 8.5 / 9.7          | 6.1 / 5.7   |
| new connections/s             | **13.8k**              | 6.2k                | 10.1k              | 11.7k       |
| p99 latency under load        | **527 µs**             | 658 µs              | 638 µs             | 1096 µs     |
| RSS with 10k idle connections | **176 MiB**            | 2770 MiB            | 784 MiB            | 708 MiB     |

With `cipher: aes-gcm` burrow reaches 22.6 / 51.5 Gbit/s. Over WebSocket it does 11.9 / 31.3 Gbit/s with 348 MiB for 10k idle connections (chisel, also WebSocket: 6.1 / 5.7 Gbit/s, 708 MiB). Unencrypted, burrow moves 2x rathole's plain TCP throughput, but opens 7% fewer connections per second (14.3k vs 15.4k), because it still authenticates every data channel with Noise.

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
