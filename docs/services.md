# Services

A **service** is one thing you expose through burrow: a port on the server that leads to a target reached by a client. Services are defined only in the server's config, under `services`. The client needs no change, and saving the file applies it.

Every option is listed in [configuration.md](configuration.md#serveryaml).

- [A public service](#a-public-service)
- [UDP](#udp)
- [Port ranges](#port-ranges)
- [Who may connect](#who-may-connect)
- [The real client address (PROXY protocol)](#the-real-client-address-proxy-protocol)
- [Several clients](#several-clients)
- [Speed and connection limits](#speed-and-connection-limits)
- [Unix sockets](#unix-sockets)
- [On the server itself](#on-the-server-itself)
- [Private services](#private-services)

## A public service

A service needs three things:

- `client`: the name of the client that reaches the target;
- `bind`: the address the server listens on for users;
- `target`: where the client connects, **as seen from the client**.

```yaml
# server.yaml, under services:
web:
  client: home
  bind: 7840                # users connect to port 7840 of the server
  target: 3000              # the client connects to 127.0.0.1:3000, on the client machine
```

`bind` and `target` can be written several ways:

```yaml
# server.yaml, under services:
app:   { client: home, bind: 7840, target: 3000 }                       # bind 0.0.0.0:7840 -> 127.0.0.1:3000 on the client
nas:   { client: home, bind: 8080, target: 192.168.1.20:80 }            # another machine on the client's network
admin: { client: home, bind: 127.0.0.1:9000, target: 9000 }             # only reachable from the server machine itself
cloud: { client: home, bind: 8443, target: nextcloud.lan:443 }          # a host name, resolved by the client
```

The client must allow each target in its `allow` list (see [configuration.md](configuration.md#clientyaml)). `burrow client-config` writes that list for you from the server's services. When you add a service later, add its target to the client's `allow` list too, or print the client config again.

## UDP

Add `protocol: udp`. Everything else is the same.

```yaml
# server.yaml, under services:
dns:
  client: home
  protocol: udp
  bind: 5353
  target: 192.168.1.1:53
```

A UDP peer that sends nothing for 60 seconds is forgotten (`udp.session_timeout_secs`). A UDP service follows its target to a new address when the target's DNS name changes.

## Port ranges

One service can expose a range of ports. Each public port leads to the same position in the target range: 30000 to 30000, 30001 to 30001, and so on. Both ranges must have the same length, at most 1000 ports.

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

`allow_from` lets only these source addresses connect. `deny_from` blocks some addresses. Both take IP addresses or CIDR ranges (`203.0.113.0/24` means 203.0.113.0 to 203.0.113.255).

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

## The real client address (PROXY protocol)

Through a tunnel, your app sees every connection coming from the burrow client, not from the real user. With `proxy_protocol: v1` (text) or `v2` (binary), the server sends the user's real address first, in the PROXY protocol that nginx, Caddy, HAProxy, Traefik and Postfix understand.

```yaml
# server.yaml, under services:
web:
  client: home
  bind: 443
  target: 8443
  proxy_protocol: v2
```

```nginx
# nginx.conf of the target app, on the client machine
server { listen 8443 ssl proxy_protocol; set_real_ip_from 127.0.0.1; real_ip_header proxy_protocol; }
```

Turn it on only when the target expects it: an app that does not understand the PROXY protocol sees garbage at the start of each connection. TCP services only.

## Several clients

List more than one client under `clients` to keep a service up when a machine goes away. This works for TCP and UDP services; `mode: balance` and `health_check` are for TCP services only, so a UDP service uses failover.

```yaml
# server.yaml, under services:
web:
  clients: [home-1, home-2]
  mode: balance          # or failover (the default)
  health_check: true     # or { interval_secs: 10, timeout_secs: 3, failures: 2 }
  bind: 443
  target: 127.0.0.1:8443
```

- **`failover`** (the default) uses the first connected client in the list, and moves to the next one while it is away.
- **`balance`** spreads new connections over all connected clients.

When a client cannot reach the target, or its `allow` list refuses it, the connection moves to the next client at once, and that client is skipped for a while. `burrow status` shows it as `failing` or `refused`. Open connections stay on their client.

**Health checks.** With `health_check`, the server also checks through each client, every 10 seconds, that the target answers. A client whose target failed 2 checks in a row gets no new connections until it answers again. So a dead target is skipped before anyone connects to it.

| health_check key | default | meaning |
| --- | --- | --- |
| `interval_secs` | `10` | seconds between checks of each client |
| `timeout_secs` | `3` | seconds a check may take |
| `failures` | `2` | failed checks in a row before a client counts as down |

## Speed and connection limits

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

## Unix sockets

`target: unix:/path` makes the client connect to a Unix socket instead of a TCP port, for example Docker's socket or PostgreSQL's.

```yaml
# server.yaml, under services:
docker:
  client: home
  target: unix:/var/run/docker.sock
  visitors: [laptop]
```

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

`client: self` makes the server connect to the target itself, with no client. Use it for something that runs next to the server, like a database, a web app, or a machine on the server's own network.

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

A service without `bind` opens no port on the server at all. Only the visitors it lists can reach it, from their own machines. See [visitors.md](visitors.md).

```yaml
# server.yaml, under services:
db:
  client: home
  target: 5432        # no bind: private
  visitors: [laptop]
```
