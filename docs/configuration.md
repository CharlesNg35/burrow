# Configuration

Everything burrow does is written in a config file, and nothing runs that the file does not ask for. This page explains how the files are written, how changes are applied, and lists every key with its default. Read it when you write a config by hand or look up what a key does; the other pages show the keys at work for each task.

Each machine has one file, for its role:

- **server.yaml** for the server, the machine with a public address. It lists the clients, the visitors and the services, and decides what is exposed.
- **client.yaml** for a client, a machine behind NAT (a home or office router) that reaches your apps.
- **visitor.yaml** for a visitor, one of your own machines (a laptop, for example) that uses private services. See [visitors.md](visitors.md).

You rarely write client.yaml or visitor.yaml by hand: `burrow client-config` and `burrow visitor-config` print them from the server's config. The words used here are explained in the [glossary](glossary.md).

## Format

The rules every config file follows:

- YAML (`.yaml`, `.yml`) or JSON (`.json`). Both use the same keys.
- `${VAR}` is replaced by the environment variable `VAR`, so secrets can stay out of the file. An unset variable is an error. Write `$${` for a literal `${`.
- In YAML, a `${VAR}` inside `{ }` must be quoted: `{ secret: "${HOME_SECRET}" }`. On a line of its own, like `secret: ${HOME_SECRET}`, no quotes are needed.
- Unknown keys are errors, so a typo never goes unnoticed.
- Relative file paths, such as certificate files, are relative to the folder of the config file.
- When you leave out `-c`, burrow uses `server.yaml`, `client.yaml` or `visitor.yaml` in the current folder.

These commands help you write a config:

```sh
./burrow validate -c server.yaml            # check a config without starting: exit 0 valid, 1 invalid, 2 usage error
./burrow validate --strict                  # also fail on warnings
./burrow schema server > server.schema.json # JSON Schema, for completion in your editor
./burrow edit                               # open a config in your editor; it is checked, saved and applied
```

`validate` reports every problem at once, with its line and column, and suggests a fix:

```
$ ./burrow validate
error: missing field "target" in service "web"
  --> server.yaml:6:3
   |
 6 |   web:
   |   ^^^ "target" is required here
   = help: add "target: ..." (host:port as seen from the client, or a port alone for 127.0.0.1 on the client)

error: unknown field "trget" in service "web"
  --> server.yaml:9:5
   |
 9 |     trget: 127.0.0.1:3000
   |     ^^^^^ unknown field
   = help: did you mean "target"? valid fields: client, clients, mode, health_check, bind, target, protocol, max_connections, visitors, allow_from, deny_from, proxy_protocol, bandwidth

server.yaml: 2 errors, 0 warnings
```

The same checks run when burrow starts and each time a config is reloaded. A config that does not pass is not applied, and the running one is kept.

## Short forms

Addresses can be written in full, or shortened to save typing. In each line below, the short form and the comment mean the same thing:

```yaml
# server.yaml
listen: 7835                 # same as 0.0.0.0:7835: all IPv4 addresses of the server
services:
  web:
    client: home
    bind: 7840               # same as 0.0.0.0:7840
    target: 3000             # same as 127.0.0.1:3000, on the client
  nas:
    client: home
    bind: 8080
    target: 192.168.1.20:80  # another machine on the client's network
```

```yaml
# client.yaml
server: myserver.example.com   # same as myserver.example.com:7835
```

```yaml
# visitor.yaml
forwards:
  db: { service: db, local: 5432 }  # local: 5432 is the same as 127.0.0.1:5432, on the visitor
```

`target` is always seen from the client: `127.0.0.1` there means the client machine itself, not the server.

## Reloading

You never need to restart burrow to change what it does. Saving a config applies it, on the server, the client and the visitor. You can also send `SIGHUP`, or run `burrow service reload` for an installed service.

- **Server:** services, clients and visitors that changed are updated. Open connections keep running, unless the change takes away their access. Only a change to `listen` needs a restart.
- **Client:** a change to `allow`, `server`, `name`, `secret`, `headers`, `ca`, `cipher`, `proxy` or `insecure_plaintext` reconnects it. Other changes apply at once, `max_connections` and `max_udp_peers` included: a new limit counts for new connections and new UDP peers. Open connections keep running, except those whose target the new `allow` no longer permits.
- **Visitor:** only the forwards that changed are opened or closed.

Files are also checked again every 30 seconds, so changes through a Docker file mount or a network share are applied too.

## server.yaml

The server's file holds the port clients connect to, the clients and visitors with their secrets, and the services. This one has a public web app, a public UDP service and a private database:

```yaml
# server.yaml
listen: 0.0.0.0:7835                 # where clients connect
public_address: myserver.example.com # optional: how clients reach this server, used by client-config
clients:
  home:
    secret: ${HOME_SECRET}           # from ./burrow gensecret
visitors:
  laptop:
    secret: ${LAPTOP_SECRET}
services:
  web:
    client: home
    bind: 0.0.0.0:7840
    target: 127.0.0.1:3000
  dns:
    client: home
    protocol: udp
    bind: 5353
    target: 192.168.1.1:53
  db:
    client: home
    target: 5432                     # no bind: private, only for visitors
    visitors: [laptop]
```

<details>
<summary><b>All server.yaml keys</b></summary>

| key | default | meaning |
| --- | --- | --- |
| `listen` | none | where the server waits for clients: ip:port, or a port alone for `0.0.0.0:port`. Needs `listen`, `websocket` or both |
| `public_address` | none | how clients reach this server: host, host:port or a `wss://` URL. `client-config` and `visitor-config` write it into the configs they print |
| `websocket.listen` | none | accept clients over WebSocket on this address; a port alone means `0.0.0.0`, or, when it shares the port of a bare `web.https`, every IPv4 and IPv6 address (`[::]`) like `web.https`. See [websocket.md](websocket.md) and [The tunnel on the same port](web/tunnel.md#the-tunnel-on-the-same-port) |
| `websocket.path` | `/burrow` | URL path for WebSocket clients |
| `websocket.host` | none | the tunnel's name when `websocket.listen` is the same port as `web.https`; required then. See [The tunnel on the same port](web/tunnel.md#the-tunnel-on-the-same-port) |
| `websocket.challenge` | automatic | the Let's Encrypt check for `websocket.host`, like a site's `challenge` |
| `websocket.tls.cert` / `key` | none | serve `wss://` directly with these PEM files |
| `websocket.keepalive_secs` | `25` | keepalive on idle connections, 5 to 30 |
| `clients.<name>.secret` | required | a 256-bit secret (base64 or hex), or a list of up to 4 while you change it |
| `clients.<name>.max_connections` | `limits.max_connections_per_client` | open forwarded connections for this client |
| `visitors.<name>.secret` | required | like a client secret |
| `visitors.<name>.reach` | none | clients this visitor may reach directly, over TCP and UDP: `CLIENT: any`, or rules like `CLIENT: [192.168.1.0/24:22]`. See [visitors.md](visitors.md) |
| `visitors.<name>.max_connections` | unlimited | open connections for this visitor; a UDP visit counts as one, whatever its number of peers |
| `visitors.<name>.admin` | none | `read` or `write`: may use `burrow admin`. See [admin.md](admin.md) |
| `visitors.<name>.admin_from` | any address | source IPs or CIDRs that admin access is limited to |
| `services.<name>.client` | required (or `clients`) | the client that reaches the target; `self` for the server itself |
| `services.<name>.clients` | | several clients instead of one. See [services.md](services.md#several-clients) |
| `services.<name>.mode` | `failover` | with several clients: `failover` or `balance` |
| `services.<name>.health_check` | off | `true`, or `{ interval_secs: 10, timeout_secs: 3, failures: 2, path: /healthz }`; `path` (websites only) asks the app for that page, 2xx or 3xx is up |
| `services.<name>.target` | required | host:port as seen from the client, a port alone (`127.0.0.1`), a range like `30000-30010`, or `unix:/path` |
| `services.<name>.bind` | none (private) | public port, range or ip:port on the server; a port alone means `0.0.0.0` |
| `services.<name>.protocol` | `tcp` | `tcp` or `udp` |
| `services.<name>.visitors` | none | visitors that may reach this service, TCP or UDP |
| `services.<name>.allow_from` | everyone | only these source IPs or CIDRs may connect |
| `services.<name>.deny_from` | nobody | these source IPs or CIDRs may never connect |
| `services.<name>.proxy_protocol` | off | `v1` or `v2`: send the real client address to the target |
| `services.<name>.bandwidth` | unlimited | speed limit per direction, for example `40mbit` or `5MB` (per second) |
| `services.<name>.max_connections` | unlimited | open connections for this TCP service |
| `services.<name>.web` | none | names that lead to this TCP service as a website: a name, a list of names, or `{ hosts, tls, https_redirect, challenge }`. See [web.md](web.md) |
| `services.<name>.web.tls` | `auto` with `web.https`, else `off` | `auto` (a `web.certificates` file, else Let's Encrypt), `{cert, key}` (the site's own files), `internal` (burrow's local CA, for LAN names; see [The local CA](web/certificates.md#the-local-ca-tls-internal)), `passthrough` (the app does HTTPS) or `off` (plain HTTP on `web.http`) |
| `services.<name>.web.tls.upload`, `target_tls.ca_upload`, `client_auth.ca_upload`, `client_auth.crl_upload`, `web.certificates[].upload`, `web.letsencrypt.ca_upload` | none | a file uploaded in the admin UI and kept in the state folder, instead of the path field beside it. See [Uploading files](web/certificates.md#uploading-files-in-the-admin-ui) |
| `services.<name>.web.https_redirect` | `web.https_redirect` | `true` sends `http://` visitors to `https://`; `false` serves plain HTTP too |
| `services.<name>.web.challenge` | automatic | the Let's Encrypt check: `tls` (port 443), `http` (port 80) or `dns` (through `web.letsencrypt.dns`). Automatic tries TLS, then HTTP |
| `services.<name>.web.paths` | all paths | only these paths on the names, each with everything below it, like `[/api]`; the longest match wins. See [Paths](web/routing.md#paths-several-apps-on-one-name) |
| `services.<name>.web.app_path` | the same path | what the app sees in place of the matched path: `/v1`, or `/` to remove the prefix; needs `paths` |
| `services.<name>.web.redirect_from` | none | names that redirect permanently to the first host, path and query kept, like `[www.example.com]` |
| `services.<name>.web.redirects` | none | paths that redirect: `/old: /new` (temporary) or `/old: {to: /new, permanent: true}`. See [Redirects](web/routing.md#redirects) |
| `services.<name>.web.maintenance` | `false` | answer every request with the maintenance page (503 with `Retry-After`) |
| `services.<name>.web.target_http2` | `false` | speak HTTP/2 to the app (h2c, or h2 inside `target_tls`), for gRPC |
| `services.<name>.web.target_tls` | off | HTTPS to the app: `true` (public CAs), `{ca: file}` (your CA) or `{insecure_skip_verify: true}` (not checked) |
| `services.<name>.web.basic_auth` | none | `user: hash` pairs, the hashes from `burrow passwd USER`; visitors log in with a password. See [Who may visit](web/access.md#passwords) |
| `services.<name>.web.forward_auth` | off | `true` asks `web.forward_auth` first, or the site's own `{service, path, copy_headers}`; not with `basic_auth`. See [Single sign-on](web/access.md#single-sign-on) |
| `services.<name>.web.rate_limit` | `web.rate_limit` | requests per second per visitor address (IPv6 per /64), bursts of twice that; 429 above |
| `services.<name>.web.client_auth` | off | `{ca, crl}`: only devices with a certificate from this CA get in. See [Device certificates](web/access.md#device-certificates) |
| `services.<name>.web.hsts`, `compress`, `max_body_size`, `response_header_timeout_secs`, `request_headers`, `response_headers`, `error_pages` | from `web` | the site's own value of the `web` key of the same name |
| `self.allow` | none | what the server may connect to for itself, for `client: self` and `reach: self`. See [services.md](services.md#on-the-server-itself) |
| `web.https` | none | the HTTPS port for websites and TLS services by name; a port alone means every IPv4 and IPv6 address, `0.0.0.0:443` IPv4 only. Off unless `web` is written. See [web.md](web.md) |
| `web.http` | none | the plain HTTP port: redirects to `https://`, plain-HTTP sites and Let's Encrypt's HTTP check. `web` needs `https`, `http` or both |
| `web.https_redirect` | `true` | the default of every HTTPS site's `https_redirect` |
| `web.http2` | `true` | browsers may use HTTP/2 on `web.https`; `false` keeps HTTP/1.1 only |
| `web.letsencrypt.email` | none | turns on Let's Encrypt certificates; writing it accepts Let's Encrypt's terms |
| `web.letsencrypt.directory` | `production` | `production`, `staging` (the test service) or the URL of another ACME server |
| `web.letsencrypt.profile` | the CA's default | a certificate profile the CA offers, like `shortlived` |
| `web.letsencrypt.ca` | none | a PEM file with the CA to trust for another ACME server |
| `web.certificates` | none | your own files: a list of `{ hosts, cert, key }`; each name is listed in one entry only |
| `web.request_header_timeout_secs` | `10` | time a visitor has to send a request's headers; also how long an idle kept-alive connection stays open |
| `web.idle_timeout_secs` | `60` | a connection that moves no data for this long is closed; WebSockets stay open |
| `web.letsencrypt.dns` | none | DNS providers for DNS checks: a list of `{ domains, cloudflare \| rfc2136 \| acme_dns \| command, wildcards, wait_secs, resolvers }`. See [DNS checks](web/certificates.md#dns-checks-wildcards-and-names-the-internet-cannot-reach) |
| `web.letsencrypt.dns[].rfc2136.algorithm` | `hmac-sha256` | the TSIG key's algorithm: `hmac-sha256`, `hmac-sha384` or `hmac-sha512`, as in the server's key file |
| `web.letsencrypt.dns[].wildcards` | none | shared wildcard certificates got through this provider, like `["*.example.com"]`; each also covers its parent name |
| `web.letsencrypt.dns[].wait_secs` | `120` | how long burrow waits for every nameserver of the zone to answer the record, 10 to 3600 |
| `web.letsencrypt.dns[].resolvers` | the system's | resolvers used only to find the zone's nameservers, like `[1.1.1.1, 9.9.9.9]` |
| `web.redirect_names` | none | names (or a name and a path) with no site of their own that only redirect, like `old.example.com: https://example.com` |
| `web.trusted_proxies` | none | address ranges of HTTP proxies whose `X-Forwarded-For` and `X-Forwarded-Proto` are believed |
| `web.proxy_protocol_from` | none | address ranges of TCP load balancers that must send a PROXY header (v1 or v2) |
| `web.access_log` | off | a file, or `{ file, format: common \| json }`: one line per request |
| `web.request_id` | `false` | an `X-Request-Id` for each request, sent to the app, returned to the visitor and logged |
| `web.hsts` | `false` | `Strict-Transport-Security` for a year on HTTPS answers |
| `web.compress` | `false` | gzip text answers when the browser accepts it |
| `web.max_body_size` | unlimited | the biggest request body, like `100MB`; above it 413 |
| `web.response_header_timeout_secs` | `60` | how long the app may take to start its answer; 504 after |
| `web.request_headers`, `web.response_headers` | none | `Name: value` set on requests to the app or on answers; `""` removes a header |
| `web.forward_auth` | none | the login service of every site with `forward_auth: true`: `{service, path, copy_headers}` |
| `web.rate_limit` | off | the default of every site's `rate_limit` |
| `web.error_pages` | built in | a folder with any of `404.html`, `413.html`, `429.html`, `502.html`, `503.html`, `504.html`, `maintenance.html` |
| `metrics.listen` | off | address for Prometheus `/metrics`; a port alone means `127.0.0.1`. See [monitoring.md](monitoring.md) |
| `insecure_plaintext` | `false` | send forwarded TCP data unencrypted on raw TCP links to clients; logging in stays encrypted |
| `heartbeat.interval_secs` | `10` | ping interval; a peer times out after 3 missed pings |
| `limits.handshake_timeout_secs` | `5` | time a new connection has to authenticate |
| `limits.max_handshakes` | `256` | connections that have not finished authenticating yet |
| `limits.max_connections_per_client` | `8192` | open forwarded connections per client, unless `clients.<name>.max_connections` sets its own |
| `limits.max_pending` | `512` | connections waiting for a free channel, per client |
| `pool.min_idle` / `pool.max_idle` | `4` / `64` | channels each client keeps open in advance, so a new connection starts at once |
| `tcp.nodelay` | `true` | send small writes at once (TCP_NODELAY) |
| `tcp.keepalive_secs` | `30` | TCP keepalive idle time; `0` turns it off |
| `udp.session_timeout_secs` | `120` | a UDP peer that sends nothing for this long is forgotten. Two minutes is the least a NAT should keep a UDP mapping (RFC 4787); a shorter value frees peers sooner for traffic like DNS, where each query may come from a new port |
| `udp.max_peers` | `4096` | most UDP peers one service port keeps at once, 1 to 1000000; a datagram from a new peer beyond it is dropped. A change applies at once to new peers |

</details>

## client.yaml

A client's file says which server to connect to, its own name and secret, and which targets it agrees to reach. `burrow client-config` writes it for you; this one shows the kinds of `allow` entries:

```yaml
# client.yaml
server: myserver.example.com:7835 # or wss://tunnel.example.com/burrow
name: home
secret: ${HOME_SECRET}
allow:
  - 127.0.0.1/32:3000      # one port on this machine
  - 192.168.1.0/24:22      # port 22 on every machine of this network
  - 192.168.1.1/32:53      # one port of one machine
  - 10.0.0.0/8:8000-8100   # a port range
  - unix:/run/postgresql/* # Unix sockets: one path, or every socket in a folder
```

`allow` is the list of targets this client agrees to connect to. Anything else is refused, even when the server asks for it. Each entry is an address range in CIDR form (`192.168.1.0/24` means 192.168.1.0 to 192.168.1.255, `/32` means one address), optionally with `:port` or `:low-high`. Host names are checked after they are resolved. `burrow client-config` writes this list from the server's services.

<details>
<summary><b>All client.yaml keys</b></summary>

| key | default | meaning |
| --- | --- | --- |
| `server` | required | host, host:port (port `7835` when left out), or a `ws://` or `wss://` URL |
| `name`, `secret` | required | this client's name and secret, as in the server's `clients` |
| `allow` | empty (nothing) | targets the server may reach: CIDR with optional `:port` or `:low-high`, or `unix:/path` |
| `allow_all` | `false` | allow any target (not recommended) |
| `cipher` | `chachapoly` | or `aes-gcm`, about 1.8 times faster on CPUs with AES instructions (most x86 CPUs) |
| `proxy` | none | connect through a proxy: `http://host:port` or `socks5://[user:pass@]host:port` |
| `insecure_plaintext` | `false` | must match the server |
| `reconnect.initial_delay_ms` | `250` | first wait before reconnecting; it doubles each time, with some randomness |
| `reconnect.max_delay_secs` | `30` | longest wait between attempts |
| `reconnect.give_up_after_secs` | `0` (never) | exit after this long without a working connection to the server |
| `reconnect.on_rejected` | `retry` | or `exit` when the server keeps refusing the name or secret for 15 s |
| `headers` | none | extra HTTP headers for the WebSocket connection, for example an access token |
| `ca` | none | an extra certificate (PEM) to trust for `wss://`: a CA that signed the server's certificate, or the server's own certificate. Certificates from public CAs are still trusted too |
| `connect_timeout_secs` | `10` | time allowed to connect to the server, 1 to 300 |
| `max_connections` | `16384` | most connections open through this client at once, 1 to 1000000; beyond it the server gets busy. Each UDP service also holds one of them for its relay. Lowered automatically when the open file limit is too small, with a warning. Each idle connection uses about 15 KB on the client, so 100000 is about 1.5 GB |
| `max_udp_peers` | `8192` | most UDP peers open through this client at once, across all UDP services, 1 to 1000000; a datagram from a new peer beyond it is dropped. Each peer uses one socket on the client, so it is lowered like `max_connections` |
| `tcp`, `udp` | as on the server | `tcp.nodelay`, `tcp.keepalive_secs`, `udp.session_timeout_secs` |

By default a client keeps retrying until it is stopped, and follows a server that moved to a new address.

</details>

## visitor.yaml

A visitor's file says which server to connect to, its own name and secret, and the forwards: local ports on your machine that lead to a service or a target. `burrow visitor-config` writes it for you; this one shows every kind of forward:

```yaml
# visitor.yaml
server: myserver.example.com
name: laptop
secret: ${LAPTOP_SECRET}
forwards:
  db:  { service: db, local: 5432 }                # a private service
  ssh: { client: home, target: 22, local: 2222 }   # a target on a client (needs reach)
  lan: { client: home, socks: true, local: 1080 }  # a SOCKS5 proxy into a client (needs reach)
  dns: { service: dns, protocol: udp, local: 5353 } # a private UDP service
```

Each forward opens a local port on the visitor's machine. See [visitors.md](visitors.md) for what each kind does.

<details>
<summary><b>All visitor.yaml keys</b></summary>

| key | default | meaning |
| --- | --- | --- |
| `server`, `name`, `secret` | required | as in client.yaml, with the visitor's name and secret |
| `forwards.<name>.service` | | a private service to reach |
| `forwards.<name>.client` | | or a client to reach directly (needs `reach` on the server) |
| `forwards.<name>.target` | | with `client`: host:port as seen from the client, or a port alone for `127.0.0.1` there |
| `forwards.<name>.socks` | `false` | with `client`: a SOCKS5 proxy, where each connection names its own target |
| `forwards.<name>.local` | required | where to listen on this machine: a port alone for `127.0.0.1`, ip:port, or a range |
| `forwards.<name>.protocol` | `tcp` | `tcp` or `udp`; must match the service's protocol on the server. A TCP and a UDP forward may use the same port number. A `unix:` target is TCP only; a SOCKS forward keeps `tcp` and carries UDP for apps that ask with UDP ASSOCIATE |
| `udp.session_timeout_secs` | `120` | a local UDP peer of a UDP forward that sends and gets nothing for this long is forgotten; the forward closes its channel to the server once it has no peers left |
| `udp.max_peers` | `4096` | most UDP peers one port of a UDP forward keeps at once; a datagram from a new peer beyond it is dropped |
| `headers`, `ca`, `cipher`, `proxy`, `connect_timeout_secs`, `tcp` | | as in client.yaml |

</details>

## Secrets

Each client and visitor logs in with its own secret. A secret is 32 random bytes, written in base64 or hex. Make one with:

```sh
./burrow gensecret
```

Never use a password: burrow refuses a secret that is not exactly 32 bytes or does not look random. To change a secret without downtime, list both on the server (`secret: [old, new]`), switch the client to the new one, then remove the old one. See [security.md](security.md).

## Next steps

- [Services](services.md): expose ports, limit who may connect, use several clients.
- [Private services and visitors](visitors.md): reach your machines from your laptop.
- [Websites](web.md): serve apps by name with HTTPS.
- [Running as a service](service.md): start burrow at boot.
