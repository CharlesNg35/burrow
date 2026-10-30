# Services

A **service** is one thing you expose through burrow: a port on the server that leads to a target reached by a client. A web app, SSH, a game server, DNS or a database are each one service. Services are defined only in the server's config, under `services`, so the server decides what is exposed. The client needs no change, and saving the file applies it.

This page shows how to expose each kind of service and how to keep it safe and up. Every option is listed in [configuration.md](configuration.md#serveryaml), and websites have [their own pages](web.md).

- [Expose an app on a public port](#expose-an-app-on-a-public-port)
- [Expose a UDP service](#expose-a-udp-service)
- [Expose a range of ports](#expose-a-range-of-ports)
- [Who may connect](#who-may-connect)
- [Pass the user's real address to the app (PROXY protocol)](#pass-the-users-real-address-to-the-app-proxy-protocol)
- [Several clients](#several-clients)
- [Limit speed and connections](#limit-speed-and-connections)
- [Reach a Unix socket](#reach-a-unix-socket)
- [On the server itself](#on-the-server-itself)
- [Private services](#private-services)

## Expose an app on a public port

A service needs three things:

- `client`: the name of the client that reaches the target;
- `bind`: the address the server listens on for users;
- `target`: where the client connects, **as seen from the client**.

This service makes the app on port 3000 of the client machine reachable on port 7840 of the server:

```yaml
# server.yaml, under services:
web:
  client: home
  bind: 7840                # users connect to port 7840 of the server
  target: 3000              # the client connects to 127.0.0.1:3000, on the client machine
```

`bind` and `target` can be written several ways, for apps on the client, on other machines of its network, or reachable only from the server itself:

```yaml
# server.yaml, under services:
app:   { client: home, bind: 7840, target: 3000 }                       # bind 0.0.0.0:7840 -> 127.0.0.1:3000 on the client
nas:   { client: home, bind: 8080, target: 192.168.1.20:80 }            # another machine on the client's network
admin: { client: home, bind: 127.0.0.1:9000, target: 9000 }             # only reachable from the server machine itself
cloud: { client: home, bind: 8443, target: nextcloud.lan:443 }          # a host name, resolved by the client
```

The client must allow each target in its `allow` list (see [configuration.md](configuration.md#clientyaml)). `burrow client-config` writes that list for you from the server's services. When you add a service later, add its target to the client's `allow` list too, or print the client config again.

## Expose a UDP service

DNS, WireGuard, game servers and other UDP services work the same way: add `protocol: udp`. This one makes the home router's DNS reachable on port 5353 of the server:

```yaml
# server.yaml, under services:
dns:
  client: home
  protocol: udp
  bind: 5353
  target: 192.168.1.1:53
```

A UDP peer that sends nothing for 2 minutes is forgotten (`udp.session_timeout_secs`). A UDP service follows its target to a new address when the target's DNS name changes.

## Expose a range of ports

Some apps, like game servers or media servers, use many ports. One service can expose a whole range of them. Each public port leads to the same position in the target range: 30000 to 30000, 30001 to 30001, and so on. Both ranges must have the same length, at most 1000 ports. This one exposes eleven UDP ports of a game server:

```yaml
# server.yaml, under services:
game:
  client: home
  protocol: udp
  bind: 30000-30010
  target: 192.168.1.50:30000-30010
```

The ranges do not need to start at the same port: `bind: 40000-40010` with `target: 30000-30010` works too.

## Who may connect

A public port is open to the whole internet unless you narrow it. `allow_from` lets only these source addresses connect. `deny_from` blocks some addresses. Both take IP addresses or CIDR ranges (`203.0.113.0/24` means 203.0.113.0 to 203.0.113.255). This SSH service is open to two networks, except one address:

```yaml
# server.yaml, under services:
ssh:
  client: home
  bind: 2222
  target: 22
  allow_from: [203.0.113.0/24, 2001:db8::/32]   # only these networks
  deny_from: [203.0.113.66]                     # but never this one address
```

Blocked connections are closed on the server and never reach your network. When you narrow these lists and save, open connections that are no longer allowed are closed. The lists also apply to visitors of a private service.

<details>
<summary><b>Behind a proxy</b></summary>

A visitor that connects through a proxy in front of `websocket.listen` (nginx, Caddy, Cloudflare) has the proxy's address, not its own. `burrow validate` warns where a service's `allow_from` or `deny_from` relies on such an address.

</details>

## Pass the user's real address to the app (PROXY protocol)

Through a tunnel, your app sees every connection coming from the burrow client, not from the real user. With `proxy_protocol: v1` (text) or `v2` (binary), the server sends the user's real address first, in the PROXY protocol that nginx, Caddy, HAProxy, Traefik and Postfix understand. Turn it on for the service:

```yaml
# server.yaml, under services:
web:
  client: home
  bind: 443
  target: 8443
  proxy_protocol: v2
```

Then tell the app to read it. For nginx on the client machine:

```nginx
# nginx.conf of the target app, on the client machine
server { listen 8443 ssl proxy_protocol; set_real_ip_from 127.0.0.1; real_ip_header proxy_protocol; }
```

Turn it on only when the target expects it: an app that does not understand the PROXY protocol sees garbage at the start of each connection. TCP services only.

## Several clients

Run the same app on two machines, and the service stays up when one of them goes away. List more than one client under `clients`. This works for TCP and UDP services; `mode: balance`, `mode: least_connections` and `health_check` are for TCP services only, so a UDP service uses failover. This web service spreads its connections over two clients and skips one whose app stops answering:

```yaml
# server.yaml, under services:
web:
  clients: [home-1, home-2]
  mode: balance          # or failover (the default), or least_connections
  health_check: true     # or { interval_secs: 10, timeout_secs: 3, failures: 2 }
  bind: 443
  target: 127.0.0.1:8443
```

- **`failover`** (the default) uses the first connected client in the list, and moves to the next one while it is away.
- **`balance`** spreads new connections over all connected clients, in turn.
- **`least_connections`** sends each new connection to the connected client with the fewest open connections. When several have the same number, the first one in the list gets it. This suits long connections, like SSH or databases, that would otherwise pile up on one machine.

All three skip a client whose health check failed, as described below.

**Weights.** With `balance`, a client can get a bigger share: write it as `{name, weight}`. The weight is a whole number from 1 to 1000, and 1 is the default. Here `home-1` gets 3 of every 4 new connections:

```yaml
# server.yaml, under services:
web:
  clients: [{name: home-1, weight: 3}, home-2]
  mode: balance
  bind: 443
  target: 127.0.0.1:8443
```

The turns are spread out evenly, so `home-1` does not get three connections in a row and then `home-2` one. This is how nginx does weighted round robin. Weights only apply to `balance`, so `validate` refuses them with the other modes.

**Several VMs behind one client.** An entry can give its own `target`, so one client in front of several machines, a gateway on a LAN with two VMs for example, can spread a service over them. Without a `target`, an entry uses the service's `target`:

```yaml
# server.yaml, under services:
app:
  clients:
    - {name: gateway, target: 10.0.0.12:3000}
    - {name: gateway, target: 10.0.0.13:3000, weight: 2}
    - home                # uses the service target below
  mode: balance
  health_check: true
  bind: 443
  target: 127.0.0.1:3000
```

Each entry is checked, counted and set aside on its own: when the VM at 10.0.0.12 stops answering, new connections go to the other VM and to `home`, and the gateway keeps serving them. The gateway's `allow` list must include each target. A client may be listed twice only with different targets, and a port range in an entry must have as many ports as the service's.

When a client cannot reach the target, or its `allow` list refuses it, the connection moves to the next client at once, and that client is skipped for a while. `burrow status` shows it as `failing` or `refused`. Open connections stay on their client.

**Health checks.** With `health_check`, the server also checks through each client, every 10 seconds, that the target answers. A client whose target failed 2 checks in a row gets no new connections until it answers again. So a dead target is skipped before anyone connects to it.

| health_check key | default | meaning |
| --- | --- | --- |
| `interval_secs` | `10` | seconds between checks of each client |
| `timeout_secs` | `3` | seconds a check may take |
| `failures` | `2` | failed checks in a row before a client counts as down |
| `path` | none | for a website: ask the app for this path, like `/healthz`, instead of only opening a connection; 2xx and 3xx count as up. See [Health checks for websites](web/health-checks.md) |

## Limit speed and connections

A busy service can fill the server's or the home connection's bandwidth. These limits keep a backup service from taking all of it:

```yaml
# server.yaml, under services:
backup:
  client: home
  bind: 8873
  target: 873
  bandwidth: 40mbit      # each direction, shared by all connections of this service
  max_connections: 20    # at most 20 open connections at a time
```

- `bandwidth` caps each direction of a service. Write it in bits per second (`kbit`, `mbit`, `gbit`, like `40mbit`) or bytes per second (`kb`, `mb`, `gb`, like `5MB`). Without it, a service is unlimited. TCP slows down to the limit; UDP datagrams over it are dropped.
- `max_connections` limits open connections of a TCP service. A client also has a limit (`limits.max_connections_per_client`, 8192 by default, or `max_connections` on the client).

## Reach a Unix socket

Some apps listen on a Unix socket instead of a TCP port, for example Docker or PostgreSQL. `target: unix:/path` makes the client connect to that socket. This makes Docker's socket on the home machine a private service for the laptop:

```yaml
# server.yaml, under services:
docker:
  client: home
  target: unix:/var/run/docker.sock
  visitors: [laptop]
```

The client must allow the socket:

```yaml
# client.yaml
allow:
  - unix:/var/run/docker.sock   # this socket, or unix:/var/run/* for every socket in that folder
```

<details>
<summary><b>Limits of Unix socket targets</b></summary>

- A socket path that is itself a symbolic link is refused; allow and use the socket it points to.
- A client installed with `burrow service` cannot see sockets under `/tmp` or in home folders, and its account must be allowed to connect to the socket. `--run-as` installs it under another account.

</details>

## On the server itself

`client: self` makes the server connect to the target itself, with no client. Use it for something that runs next to the server, like a database, a web app, or a machine on the server's own network. Here the server reaches its own database, web app and network, and shares one service with a client:

```yaml
# server.yaml
self:
  allow: [127.0.0.1/32, 10.0.0.0/24]   # what the server may connect to for itself
services:
  db:  { client: self, target: 127.0.0.1:5432, visitors: [laptop] }
  web: { client: self, bind: 8080, target: 127.0.0.1:3000 }
  app: { clients: [self, home], bind: 443, target: 127.0.0.1:8443 }
```

Everything works as with any client: public and private services, UDP, ranges, limits, the PROXY protocol, health checks, and `self` among several `clients`. Visitors reach the server's own network with `reach: self` (see [visitors.md](visitors.md#any-target-on-a-client)).

<details>
<summary><b>What <code>self.allow</code> controls</b></summary>

`self.allow` works like a client's `allow` list. Without a `self` section, a `self` service reaches only its own target, and `reach: self` is refused.

The server never connects to its own listen, WebSocket or metrics addresses, whatever `self.allow` says. It also never connects to a link-local or cloud metadata address (such as 169.254.169.254), unless `self.allow` names that exact address on its own.

</details>

## Private services

A service without `bind` opens no port on the server at all. Only the visitors it lists can reach it, from their own machines. Use it for anything that should never face the internet, like a database or SSH. See [visitors.md](visitors.md).

```yaml
# server.yaml, under services:
db:
  client: home
  target: 5432        # no bind: private
  visitors: [laptop]
```

## Next steps

- [Private services and visitors](visitors.md): use private services from your laptop.
- [Websites](web.md): serve a TCP service by name, with HTTPS.
- [Monitoring](monitoring.md): see each service's connections and traffic.
- [All service keys](configuration.md#serveryaml).
