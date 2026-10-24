# Websites on one HTTPS port

burrow can put several websites and TLS services on one HTTPS port of the server, usually 443, and tell them apart by name. A visitor types `https://nas.example.com`, the browser sends that name at the start of the TLS connection, and burrow sends the connection on to the service with that name, through the tunnel like any other service.

The burrow tunnel itself can share the same port: clients then connect over `wss://` to a name of its own, while the websites keep working beside it.

```
browser ── https://nas.example.com ──> server:443 ══ tunnel ══ client ──> NAS on 192.168.1.20:443
burrow client ── wss://tunnel.example.com/burrow ──> server:443 (the tunnel)
```

Nothing of this runs unless you write it in the config.

## Turning it on

The top-level `web:` names the HTTPS port. A service becomes a website by having its own `web:` with the names that lead to it:

```yaml
# server.yaml
listen: 7835
clients:
  home:
    secret: ${HOME_SECRET}
web:
  https: 443 # every IPv4 and IPv6 address; 0.0.0.0:443 for IPv4 only
services:
  nas:
    client: home
    target: 192.168.1.20:443
    web:
      hosts: [nas.example.com]
      tls: passthrough # the NAS does its own HTTPS
```

Point the name at the server in your DNS, open port 443 in the server's firewall, and start or save. `burrow validate` shows what will happen:

```text
server.yaml: valid server config (1 client, 1 service)
web: https [::]:443 (IPv4 and IPv6)
nas.example.com: passthrough, the app's own certificate -> service nas
```

`burrow validate --format json` gives the same under `"web"`.

- **A port alone listens on IPv4 and IPv6.** Many home connections only have a public IPv6 address. Write `0.0.0.0:443` to listen on IPv4 only, or one address, like `203.0.113.10:443`.
- **Port 443 is below 1024.** On Linux burrow needs root or `CAP_NET_BIND_SERVICE` for it; `burrow service install` takes care of that.
- **A service can have both `bind` and `web`.** The tunnel part of the service (`client`, `clients`, `target`, `allow_from`, `max_connections`, `bandwidth`, `proxy_protocol`, health checks) works for websites exactly as for a TCP service.
- **Unknown names are closed.** A connection whose name no service has, or that sends no name, is closed at once. No certificate is shown, so no name is given away.

## Passthrough

With `tls: passthrough`, burrow does not decrypt anything. It reads only the name in the first message of the connection (the TLS "ClientHello") and passes every byte to the service. The app keeps its own certificate and its own TLS setup. This suits devices that already serve HTTPS, like a NAS, a router or Proxmox, and any TLS service that is not a website.

```yaml
# server.yaml
services:
  nas:
    client: home
    target: 192.168.1.20:443
    proxy_protocol: v2 # optional: the NAS sees the visitor's address
    web:
      hosts: [nas.example.com, files.example.com]
      tls: passthrough
  apps:
    clients: [home, office] # failover between two clients, as for any service
    target: 8443
    web:
      hosts: ["*.apps.example.com"]
      tls: passthrough
```

Names:

- `hosts` takes DNS names, like `nas.example.com`, and wildcards of one label, like `*.apps.example.com`. That wildcard covers `x.apps.example.com`, not `apps.example.com` and not `a.b.apps.example.com`. An exact name in another service wins over a wildcard.
- An IP address, like `203.0.113.10`, is for visitors who type the address: browsers send no name then, so burrow uses the address they connected to.
- Each name leads to one service. Upper and lower case do not matter, and a final dot is ignored.
- Names are added and removed when you save the config. Open connections to a name that was removed are closed.

`tls` says who does HTTPS for a website:

| `tls` | Meaning | In this build |
| --- | --- | --- |
| `auto` (the default with `web.https`) | burrow does HTTPS with a certificate for every name | not yet |
| `passthrough` | the app does its own HTTPS; burrow only reads the name | yes |
| `off` | no HTTPS: plain HTTP on `web.http` | not yet |
| `{cert, key}` | burrow does HTTPS with this site's own certificate file | not yet |

The short forms `web: nas.example.com` and `web: [a.example.com, b.example.com]` use the default, `auto`. Until burrow can serve HTTPS itself, `burrow validate` refuses every form but `passthrough` and says how to fix it.

## The tunnel on the same port

When `websocket.listen` is the same port as `web.https`, the tunnel shares it. `websocket.host` is then the name that leads to the tunnel, and the tunnel needs a certificate for that name in `websocket.tls`:

```yaml
# server.yaml
websocket:
  listen: 443 # the same as web.https
  host: tunnel.example.com
  path: /burrow
  tls:
    cert: /etc/letsencrypt/live/tunnel.example.com/fullchain.pem
    key: /etc/letsencrypt/live/tunnel.example.com/privkey.pem
web:
  https: 443
clients:
  home:
    secret: ${HOME_SECRET}
services:
  nas:
    client: home
    target: 192.168.1.20:443
    web: { hosts: [nas.example.com], tls: passthrough }
```

```yaml
# client.yaml
server: wss://tunnel.example.com/burrow
```

- Write both ports the same way: `443` and `443`, or `0.0.0.0:443` and `0.0.0.0:443`. Mixing them, like `443` and `0.0.0.0:443`, is an error, since they would listen on different addresses.
- `public_address` defaults to `wss://tunnel.example.com/burrow`, so `burrow client-config` prints the right URL.
- The tunnel's name is never sent to a website, and no website request ever reaches the tunnel. A wrong path on the tunnel's name gets `404`.
- A new certificate file for the tunnel is used for new connections without a restart.
- Without `host`, `websocket.listen` is its own port as before, for example `127.0.0.1:7836` behind nginx: see [websocket.md](websocket.md).

## What validate checks

| Problem | What validate says |
| --- | --- |
| a service has `web` but there is no top-level `web` | `services.nas.web: the web proxy is off; add a top-level web: with its port, like web: {https: 443}` |
| `web` has no port | `web needs a port: https for HTTPS sites, like https: 443 (plain HTTP sites on web.http are not in this build yet)` |
| a website with `tls: auto` (or no `tls`) or `{cert, key}` | `services.blog.web.tls: burrow cannot serve HTTPS for websites in this build yet; use tls: passthrough` |
| a website with `tls: off` | `services.blog.web.tls: plain HTTP websites need web.http, which this build does not have yet; use tls: passthrough` |
| an HTTPS name with no certificate | `host "blog.example.com" (service "blog") has no certificate; add web.letsencrypt for automatic certificates, tls: {cert, key} on service "blog", or an entry in web.certificates whose hosts lists it` |
| a wildcard name from Let's Encrypt | `services.apps.web.hosts: "*.apps.example.com" needs DNS-01 (Let's Encrypt gives wildcard certificates only through DNS checks), which this build does not have yet; add a certificate file under web.certificates or tls` |
| a name that is not a host name | `services.nas.web.hosts: "https://nas.example.com" is not a host name; write the name alone, like blog.example.com` |
| a wildcard that is too wide | `services.apps.web.hosts: "*.com" is too wide; a wildcard needs a parent name, like *.example.com` |
| one name in two services | `host "nas.example.com" is used by services "nas" and "nas2"; each host name leads to one service` |
| the tunnel shares the port but has no name | `websocket.host is required when websocket.listen is the same port as web.https (443): ...` |
| `websocket.host` without sharing | `websocket.host is only used when websocket.listen is the same port as web.https; ...` |
| the tunnel's name is a passthrough site | `websocket.host "nas.example.com" is a passthrough site; burrow cannot see its requests` |
| the tunnel's name has no certificate | `websocket.host "tunnel.example.com" has no certificate; add web.letsencrypt, websocket.tls, or an entry in web.certificates whose hosts lists it` |
| a UDP service with `web` | `service "dns" is udp, but web only works for tcp services; remove web:, or set protocol: tcp` |
| another listener on the same port | `web.https [::]:443 is also the control port (listen ...)`, or the service or metrics port it clashes with |

## Limits

- A connection has 10 seconds to send its first TLS message, and at most 16 KiB of it is read before the name is known.
- Connections that have not shown a name yet are limited like the tunnel's own handshakes (`limits.max_handshakes`), with a share per address, but with their own counter: a flood of web connections cannot block tunnel clients, and the other way round.
- A tunnel connection on the shared port has `limits.handshake_timeout_secs` in total for TLS, the WebSocket upgrade and burrow's own handshake.
