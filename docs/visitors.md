# Private services and visitors

A **visitor** is one of your own machines, such as your laptop, that reaches things behind a client without exposing them to the internet. The visitor connects to the server with its own name and secret, opens ports on its own machine, and burrow carries each connection to the client:

```
your laptop: 127.0.0.1:5432 ══ server ══ client (behind NAT) ──> 127.0.0.1:5432 on the client
```

A visitor can use:

- **[private services](#private-services):** services without a public port, that list the visitor;
- **[any target on a client](#any-target-on-a-client)** you allow with `reach`, like `ssh -L`;
- **[a SOCKS5 proxy](#a-socks5-proxy-into-a-client)** into a client, so each app picks its own target.

A forward carries TCP by default, or UDP with `protocol: udp`: to a private UDP service, or to a UDP target on a client through `reach`. See [UDP forwards](#udp-forwards).

A visitor can also be used for the web admin only. See [admin.md](admin.md).

## Private services

A service without `bind` opens no port on the server. Only the visitors listed in its `visitors` can reach it.

**1. In server.yaml,** add a visitor with its own secret (from `./burrow gensecret`), and list it on the service:

```yaml
# server.yaml
visitors:
  laptop:
    secret: ${LAPTOP_SECRET}
services:
  db:
    client: home
    target: 5432          # 127.0.0.1:5432 on the client
    visitors: [laptop]    # no bind: private
```

**2. On the server,** print the visitor's config:

```sh
./burrow visitor-config laptop --server myserver.example.com -o visitor.yaml
```

`--server` is the server's public name or IP address, as the visitor reaches it. You can leave it out when server.yaml sets `public_address`.

```yaml
# visitor.yaml
server: myserver.example.com:7835
name: laptop
secret: ...
forwards:
  db:
    service: db
    local: 5432           # a port alone means 127.0.0.1:5432 on the visitor
```

**3. On your laptop,** put `visitor.yaml` next to burrow and run:

```sh
./burrow visit         # runs every forward of ./visitor.yaml
```

Now `127.0.0.1:5432` on your laptop leads to the database: `psql -h 127.0.0.1 -p 5432`.

`burrow visit` can also run only part of the config, or something that is not in it:

```sh
./burrow visit                                # every forward in ./visitor.yaml
./burrow visit db                             # only the forward named db
./burrow visit db --local 5433                # only db, on local port 5433 instead
./burrow visit --service db --local 5432      # a private service, without a forward in the config
./burrow visit -c work.yaml                   # another config file
```

With `--service`, `--client` or `--socks`, the forwards in the config are not started; only the one on the command line runs. The config still gives the server, the name and the secret.

<details>
<summary><b>Without access to the server's config</b></summary>

`./burrow init visitor` asks for the server's address, the visitor's name and its secret. It then asks the server what this visitor may use, and writes the forwards for it:

```sh
./burrow init visitor            # writes visitor.yaml
./burrow init visitor --update   # later: ask the server again and update the forwards
```

`--update` shows the changes and asks before writing. It keeps your local ports and the rest of the file.

</details>

## Any target on a client

With `reach`, a visitor picks the target itself, without a service for each one. This is like `ssh -L`.

```yaml
# server.yaml
visitors:
  laptop:
    secret: ${LAPTOP_SECRET}
    reach:
      home: [127.0.0.1/32:22, 192.168.150.0/24]   # only these targets on client home
      office: any                                  # anything client office allows
```

`reach` takes, for each client:

- `any`: any target that client's `allow` list permits;
- rules in the same form as the `allow` list: an address range in CIDR form (`192.168.150.0/24` means 192.168.150.0 to 192.168.150.255, `/32` means one address), optionally with `:port` or `:low-high`.

**The client must allow the target too.** The client's `allow` list always has the final say, and `burrow client-config` only puts the targets of services into it. Add the targets you want visitors to reach:

```yaml
# client.yaml of client home
allow:
  - 127.0.0.1/32:22
  - 192.168.150.0/24
```

Then, on the visitor, name the client and the target:

```sh
./burrow visit --client home --target 22 --local 2222                  # --target 22 means 127.0.0.1:22 on the client; then: ssh -p 2222 localhost
./burrow visit --client home --target 192.168.150.44:22 --local 2222   # a machine on the client's network; then: ssh -p 2222 localhost
./burrow visit --client home --target 192.168.150.3:3000 --local 3000  # a web app on another machine; then open http://localhost:3000
```

The same in visitor.yaml, all running at once (each on its own local port):

```yaml
# visitor.yaml
server: myserver.example.com
name: laptop
secret: ${LAPTOP_SECRET}
forwards:
  home-ssh: { client: home, target: 22, local: 2222 }
  pc-ssh:   { client: home, target: 192.168.150.44:22, local: 2223 }
  web:      { client: home, target: 192.168.150.3:3000, local: 3000 }
```

`target` is always seen from the client: `127.0.0.1` there is the client machine. A host name is resolved by the client, on its own network, and then checked against the rules.

`reach: { self: ... }` lets a visitor reach the server's own machine and network, when the server has a `self.allow` list. See [services.md](services.md#on-the-server-itself).

## A SOCKS5 proxy into a client

Instead of one forward per target, open one local SOCKS5 port, and let each app name its own target. SOCKS5 is a standard proxy protocol that browsers, curl, ssh and many other apps support. It needs `reach` for that client, as above.

```sh
./burrow visit --client home --socks 1080    # a SOCKS5 proxy on 127.0.0.1:1080
```

```yaml
# visitor.yaml, under forwards:
home: { client: home, socks: true, local: 1080 }
```

Then point an app at `127.0.0.1:1080`:

```sh
curl --socks5-hostname 127.0.0.1:1080 http://nas.local:5000    # the name nas.local is resolved by the client
ssh -o ProxyCommand='nc -X 5 -x 127.0.0.1:1080 %h %p' 192.168.150.44
```

In a browser, set the SOCKS5 proxy to `127.0.0.1`, port `1080`, and turn on "proxy DNS when using SOCKS v5", so names are looked up on the client's network.

`burrow visitor-config` writes one SOCKS forward for each client in the visitor's `reach`, on local port 1080, then 1081 for the next client, and so on.

<details>
<summary><b>What the SOCKS5 proxy supports</b></summary>

- **TCP only.** The proxy supports the CONNECT command, which is what apps use to open a TCP connection. UDP ASSOCIATE (UDP through the proxy) and BIND are not supported, because visitors are TCP only. A tool that tests UDP reports that the proxy does not support UDP; that is expected. Set the app to use the proxy for TCP only.
- **No user name or password.** The proxy listens on `127.0.0.1` by default, so only your own machine reaches it. An app that insists on a user name and password is refused.
- **Targets:** IPv4 and IPv6 addresses, and host names. A host name is resolved by the client.
- When a target cannot be reached, the app gets a standard SOCKS5 error:

  | reason | SOCKS5 reply |
  | --- | --- |
  | the target is not in `reach` or not in the client's `allow` list, or the client cannot resolve the host name | connection not allowed by ruleset (code 2) |
  | the client is not connected | network unreachable (code 3) |
  | the target refuses or does not answer | connection refused (code 5) |
  | the client or server is busy, or the request is invalid | general failure (code 1) |
  | the app asks for UDP ASSOCIATE or BIND | command not supported (code 7) |
  | the app offers only a user name and password | no acceptable methods |

</details>

### If a SOCKS app says it cannot connect

Some apps show only a vague error, such as "code -1", whatever went wrong. Look at the visitor's log: it says what the app asked for and which SOCKS5 reply it got.

- **Most often, the target is not allowed.** The app gets "connection not allowed by ruleset" (code 2). Add the target to the client's `allow` list, and to the visitor's `reach` if you use rules there. The client's log names the exact rule to add. Some apps look up a host name themselves and send its IP address, so it is that address that must be allowed.
- **A test of UDP fails.** That is expected: visitors are TCP only, so UDP ASSOCIATE gets "command not supported" (code 7). TCP through the proxy still works.
- **The app asks for a user name and password.** Set it to no authentication.

## UDP forwards

```yaml
# visitor.yaml, under forwards:
dns: { service: dns, protocol: udp, local: 5353 }
```

`protocol` must match the service's protocol on the server; if it does not, `burrow visit` says which one to set. For a quick try, `burrow visit --udp --service dns --local 5353` does the same without a config. A TCP and a UDP forward may use the same local port number. The first datagram opens one channel to the server for each local port; it closes once no local peer has sent or received anything for `udp.session_timeout_secs` (2 minutes by default), and the next datagram opens it again. A port keeps at most `udp.max_peers` local peers (4096). A server older than v0.1.2 cannot carry UDP for visitors.

A UDP forward that listens on an address other than 127.0.0.1 accepts datagrams from any source address, including forged ones, since UDP has no handshake: anyone on that network can fill its `udp.max_peers` and make it send replies to an address of their choice. Keep `local` on 127.0.0.1 unless the network is trusted.

## Port ranges

A forward can open a range of local ports, for a private TCP or UDP service with a port range on the server. Both ranges must have the same size.

```yaml
# visitor.yaml, under forwards:
games: { service: games, local: 40000-40010 }
```

## Keeping a visitor running

```sh
sudo ./burrow service install -c visitor.yaml
```

See [service.md](service.md). A visitor applies its config on save, like the server and the client: only the forwards that changed are opened or closed.

## Good to know

- `burrow validate` on the server warns about `reach: any`, as a reminder to keep the client's `allow` list narrow.
- Removing a visitor, its secret, or its access from server.yaml and saving closes its open connections at once.
- A service's `allow_from` and `deny_from` also apply to visitors, with the visitor's source address.
- `visitors.<name>.max_connections` limits the open connections of one visitor.
