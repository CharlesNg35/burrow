# Configuration

burrow has three kinds of config files, one for each role:

- **server.yaml** for the server, the machine with a public address. It lists the clients, the visitors and the services.
- **client.yaml** for a client, a machine behind NAT that reaches your apps.
- **visitor.yaml** for a visitor, one of your own machines (a laptop, for example) that uses private services. See [visitors.md](visitors.md).

You rarely write client.yaml or visitor.yaml by hand: `burrow client-config` and `burrow visitor-config` print them from the server's config.

## Format

- YAML (`.yaml`, `.yml`) or JSON (`.json`). Both use the same keys.
- `${VAR}` is replaced by the environment variable `VAR`, so secrets can stay out of the file. An unset variable is an error. Write `$${` for a literal `${`.
- Unknown keys are errors, so a typo never goes unnoticed.
- Relative file paths, such as certificate files, are relative to the folder of the config file.
- When you leave out `-c`, burrow uses `server.yaml`, `client.yaml` or `visitor.yaml` in the current folder.

```sh
./burrow validate -c server.yaml            # check a config without starting: exit 0 valid, 1 invalid, 2 usage error
./burrow validate --strict                  # also fail on warnings
./burrow schema server > server.schema.json # JSON Schema, for completion in your editor
./burrow edit                               # open a config in your editor; it is checked, saved and applied
```

`validate` reports every problem with its line and column, and suggests a fix:

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

Addresses can be written in full, or shortened. These pairs mean the same thing:

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

Saving a config applies it, on the server, the client and the visitor. You can also send `SIGHUP`, or run `burrow service reload` for an installed service.

- **Server:** services, clients and visitors that changed are updated. Open connections keep running, unless the change takes away their access. Only a change to `listen` needs a restart.
- **Client:** a change to `allow`, `server`, `name`, `secret`, `headers`, `ca`, `cipher`, `proxy` or `insecure_plaintext` reconnects it. Other changes apply at once, `max_connections` and `max_udp_peers` included: a new limit counts for new connections and new UDP peers. Open connections keep running, except those whose target the new `allow` no longer permits.
- **Visitor:** only the forwards that changed are opened or closed.

Files are also checked again every 30 seconds, so changes through a Docker file mount or a network share are applied too.

## server.yaml

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
| `websocket.listen` | none | accept clients over WebSocket on this address; a port alone means `0.0.0.0`. See [websocket.md](websocket.md) |
| `websocket.path` | `/burrow` | URL path for WebSocket clients |
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
| `services.<name>.health_check` | off | `true`, or `{ interval_secs: 10, timeout_secs: 3, failures: 2 }` |
| `services.<name>.target` | required | host:port as seen from the client, a port alone (`127.0.0.1`), a range like `30000-30010`, or `unix:/path` |
| `services.<name>.bind` | none (private) | public port, range or ip:port on the server; a port alone means `0.0.0.0` |
| `services.<name>.protocol` | `tcp` | `tcp` or `udp` |
| `services.<name>.visitors` | none | visitors that may reach this service, TCP or UDP |
| `services.<name>.allow_from` | everyone | only these source IPs or CIDRs may connect |
| `services.<name>.deny_from` | nobody | these source IPs or CIDRs may never connect |
| `services.<name>.proxy_protocol` | off | `v1` or `v2`: send the real client address to the target |
| `services.<name>.bandwidth` | unlimited | speed limit per direction, for example `40mbit` or `5MB` (per second) |
| `services.<name>.max_connections` | unlimited | open connections for this TCP service |
| `self.allow` | none | what the server may connect to for itself, for `client: self` and `reach: self`. See [services.md](services.md#on-the-server-itself) |
| `metrics.listen` | off | address for Prometheus `/metrics`; a port alone means `127.0.0.1`. See [monitoring.md](monitoring.md) |
| `insecure_plaintext` | `false` | send forwarded TCP data unencrypted on raw TCP links to clients; logging in stays encrypted |
| `heartbeat.interval_secs` | `10` | ping interval; a peer times out after 3 missed pings |
| `limits.handshake_timeout_secs` | `5` | time a new connection has to authenticate |
| `limits.max_handshakes` | `256` | connections that have not finished authenticating yet |
| `limits.max_connections_per_client` | `8192` | open forwarded connections per client, unless the client sets its own |
| `limits.max_pending` | `512` | connections waiting for a free channel, per client |
| `pool.min_idle` / `pool.max_idle` | `4` / `64` | channels each client keeps open in advance, so a new connection starts at once |
| `tcp.nodelay` | `true` | send small writes at once (TCP_NODELAY) |
| `tcp.keepalive_secs` | `30` | TCP keepalive idle time; `0` turns it off |
| `udp.session_timeout_secs` | `60` | a UDP peer that sends nothing for this long is forgotten |
| `udp.max_peers` | `4096` | most UDP peers one service port keeps at once, 1 to 1000000; a datagram from a new peer beyond it is dropped. A change applies at once to new peers |

</details>

## client.yaml

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
| `forwards.<name>.protocol` | `tcp` | `tcp` or `udp`; must match the service's protocol on the server. A TCP and a UDP forward may use the same port number. A SOCKS forward and a `unix:` target are TCP only |
| `udp.session_timeout_secs` | `60` | a local UDP peer of a UDP forward that sends and gets nothing for this long is forgotten; the forward closes its channel to the server once it has no peers left |
| `udp.max_peers` | `4096` | most UDP peers one port of a UDP forward keeps at once; a datagram from a new peer beyond it is dropped |
| `headers`, `ca`, `cipher`, `proxy`, `connect_timeout_secs`, `tcp` | | as in client.yaml |

</details>

## Secrets

A secret is 32 random bytes, written in base64 or hex. Make one with:

```sh
./burrow gensecret
```

Never use a password: burrow refuses a secret that is not exactly 32 bytes or does not look random. To change a secret without downtime, list both on the server (`secret: [old, new]`), switch the client to the new one, then remove the old one. See [security.md](security.md).
