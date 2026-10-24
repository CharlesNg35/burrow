# Websites on one HTTPS port

burrow can serve your apps as websites. A visitor types `https://blog.example.com`, the server receives the request on port 443, and burrow sends it through the tunnel to the app on your client, like any other service. Many websites share one port and are told apart by their names. burrow can do HTTPS itself, with certificates from Let's Encrypt or your own files, or pass the encrypted connection to an app that does its own HTTPS.

```
browser ── https://blog.example.com ──> server:443 ══ tunnel ══ client ──> app on 127.0.0.1:8080
burrow client ── wss://tunnel.example.com/burrow ──> server:443 (the tunnel can share the port)
```

Nothing of this runs unless you write it in the config.

## A first website

```yaml
# server.yaml
listen: 7835
clients:
  home:
    secret: ${HOME_SECRET}
web:
  https: 443
  http: 80
  letsencrypt:
    email: ops@example.com # writing this accepts Let's Encrypt's terms
services:
  blog:
    client: home
    target: 8080 # the app on the client machine, port 8080
    web: blog.example.com
```

1. Point `blog.example.com` at the server in your DNS (an `A` or `AAAA` record).
2. Open ports 443 and 80 in the server's firewall.
3. Start the server, or save the config if it runs already.

burrow gets a certificate for `blog.example.com` from Let's Encrypt within a minute or so, and the site is up. Until then, a visitor on `http://` sees a short page saying the certificate is on its way. `burrow validate` shows what will happen:

```text
server.yaml: valid server config (1 client, 1 service)
web: https [::]:443 (IPv4 and IPv6), http [::]:80 (IPv4 and IPv6)
certificates: Let's Encrypt (production) as ops@example.com
blog.example.com: Let's Encrypt, TLS check -> service blog
http [::]:80: other HTTPS hosts redirect to https:// (308); Let's Encrypt HTTP checks answered
```

`burrow init server` can write this for you: answer yes to its website question, or pass `--web blog.example.com --email ops@example.com`.

## The ports

| Key | What it does |
| --- | --- |
| `web.https` | HTTPS for every website, and the TLS services passed through by name. |
| `web.http` | Plain HTTP: it sends `http://` visitors to `https://`, serves plain-HTTP websites, and answers Let's Encrypt's HTTP check. |

- A port alone, like `443`, listens on IPv4 and IPv6. Many home connections only have a public IPv6 address. Write `0.0.0.0:443` for IPv4 only, or one address, like `203.0.113.10:443`.
- `web:` needs at least one of the two. Without `web.https`, every website is plain HTTP.
- Ports below 1024 need root or `CAP_NET_BIND_SERVICE` on Linux; `burrow service install` takes care of that.
- Without `web.http`, nothing answers `http://`: a visitor who types `http://blog.example.com` gets "connection refused".

### Port forwarding at home

If the server is behind a home router, forward the internet's port 443 (and 80) to the same ports on the server. If the router forwards 443 to another port, like 8443, burrow cannot see that. Let's Encrypt checks a name only on port 443 or port 80 of the internet side, so with `web.https: 8443` validate asks you to forward port 80 to `web.http: 80` too, or to use your own certificate files:

```yaml
# server.yaml (the router forwards 443 to 8443 and 80 to 80)
web:
  https: 8443
  http: 80
  letsencrypt:
    email: ops@example.com
```

## How a website is served

A service becomes a website by having its own `web:`. The short forms `web: blog.example.com` and `web: [example.com, www.example.com]` list the names. The long form says more:

```yaml
# server.yaml
services:
  blog:
    client: home
    target: 8080
    web:
      hosts: [example.com, www.example.com]
      tls: auto # the default when web.https is set
      https_redirect: true # the default: http:// goes to https://
```

`tls` says who does HTTPS:

| `tls` | What happens |
| --- | --- |
| `auto` | burrow does HTTPS with a certificate for every name: from a file in `web.certificates` that lists the name, else from Let's Encrypt. The default with `web.https`. |
| `{cert: ..., key: ...}` | burrow does HTTPS with this site's own certificate file, which must cover every name of the site. |
| `passthrough` | The app does its own HTTPS. burrow only reads the name and passes the encrypted bytes on. |
| `off` | No HTTPS: the site is plain HTTP on `web.http`. The default without `web.https`. |

`https_redirect` decides what an HTTPS site does on `web.http`: `true` (the default) sends visitors to the same address on `https://` with a permanent redirect (308); `false` answers plain HTTP too. Set it for every site under `web:`, or per site.

Names:

- `hosts` takes DNS names, like `blog.example.com`, wildcards of one label, like `*.apps.example.com`, and IP addresses, like `203.0.113.10`.
- A wildcard covers exactly one label: `*.apps.example.com` covers `x.apps.example.com`, not `apps.example.com` and not `a.b.apps.example.com`. An exact name in another service wins over a wildcard.
- Each name leads to one service. Upper and lower case do not matter, and a final dot is ignored.
- A name without a dot, like `nas`, is allowed only on a plain-HTTP site (`tls: off`). HTTPS needs a full name, like `nas.home.example.com`: browsers send only full names when they connect, and no public CA issues certificates for short ones.
- Names are added and removed when you save the config.

### What burrow does for a website

- **Requests go to the app through the tunnel**, one connection per visitor connection. WebSocket upgrades work.
- **The app learns who asked:** burrow sets `X-Forwarded-For`, `X-Forwarded-Proto`, `X-Forwarded-Host`, `X-Forwarded-Port` and `X-Real-IP`. Headers a visitor sends with these names are replaced.
- **The name must match:** a request whose `Host` differs from the name of the HTTPS connection gets `421`, and a request without `Host` gets `400`.
- **Pages when something is wrong**, instead of a broken connection: `503` when no client of the service is connected, all are down or the service is at its connection limit, `502` when the client cannot reach the app, `404` for an unknown name on port 80. A failure after the app started answering closes the connection.
- **Time limits:** a visitor has 10 seconds to send a request's headers (`web.request_header_timeout_secs`, also how long an idle kept-alive connection stays open), and a connection that moves no data for 60 seconds is closed (`web.idle_timeout_secs`; WebSockets stay open).
- **TLS 1.2 and 1.3.** TLS 1.0 and 1.1 are retired (RFC 8996); clients that only speak them cannot reach any site on the port, passthrough sites included.
- Everything else a service does works for websites too: several clients with failover or balance, health checks, `allow_from`, `max_connections`, `bandwidth`.

## Certificates

For each name of an HTTPS site, burrow takes the first that applies:

1. the site's own `tls: {cert, key}`;
2. an entry in `web.certificates` whose `hosts` lists the name (a listed wildcard covers the names one label below it);
3. `web.letsencrypt`.

If none applies, validate refuses the name and says how to fix it.

### Your own files

```yaml
# server.yaml
web:
  https: 443
  certificates:
    - hosts: ["*.example.com", example.com]
      cert: certs/example.com/fullchain.pem
      key: certs/example.com/privkey.pem
    - hosts: [nas.home.arpa] # an internal name, from your own CA
      cert: certs/home.pem
      key: certs/home.key
```

- Paths are relative to the config file. A file is used only for the names its `hosts` lists, and each name may be listed in one entry only.
- burrow checks that the key matches the certificate and that the certificate covers each listed name.
- A file that changes on disk, for example after `certbot renew`, is used from the next reload; a file that breaks keeps the last good certificate in use.
- burrow warns 14 days before a file expires.

### Let's Encrypt

```yaml
# server.yaml
web:
  https: 443
  http: 80
  letsencrypt:
    email: ops@example.com
    # directory: staging          # Let's Encrypt's test service, for trying things out
    # profile: shortlived         # 6-day certificates
    # directory: https://ca.internal/acme/directory  # another ACME server
    # ca: certs/internal-ca.pem   # the CA to trust for that server
```

- **Writing `letsencrypt:` with an email accepts Let's Encrypt's terms** (<https://letsencrypt.org/repository/>). Let's Encrypt stopped sending expiry emails in June 2025; burrow shows the state of every certificate in `burrow status`, the admin UI and its metrics instead.
- **How Let's Encrypt checks a name:** over TLS on port 443 when `web.https` is 443, else over HTTP on port 80 when `web.http` is 80. Each name needs one of them reachable from the internet, also from several places, so a firewall that allows only some countries or addresses breaks the check.
- **Wildcards** need a DNS check, and DNS checks come in a later phase: use your own file for them.
- **Many sites under one domain** are best served by one wildcard certificate, like `*.example.com`: today as your own file in `web.certificates` (see the recipe below), and from Let's Encrypt with a DNS check once that phase lands.
- **IP addresses** get Let's Encrypt's short-lived certificates (about 6 days), renewed after half their life.
- **`profile`** applies to certificates for DNS names; IP addresses always get the short-lived profile (or none if the CA doesn't list it). burrow checks the profile against the list the CA offers before ordering; one it does not offer is shown in `burrow status` and the admin UI with the list of the offered ones.
- **Renewal** happens by itself, at the time Let's Encrypt suggests (ARI), else after two thirds of a certificate's life. A failed try is retried after 1 minute, 10 minutes, 100 minutes, then daily, and the old certificate keeps being served, even after it expires.
- **No duplicates:** certificates are kept by their names. A restart, a renamed service or a name moved to another service orders nothing new, and at most 4 new certificates per set of names are ordered in 7 days.
- **Where they are kept:** a private folder that `burrow status` names. An installed service keeps it in `/var/lib/burrow/<name>` on Linux, `/Library/Application Support/burrow/<name>` on macOS, `%ProgramData%\burrow\services\<name>` on Windows; `burrow service uninstall` keeps it and only `--purge` deletes it. A server run by hand uses one folder per config file; `burrow server --state-dir DIR` picks another.
- **The account** is shown in `burrow status`, for a CAA DNS record with `accounturi` that lets only this account get certificates for your domain.

### When a certificate fails

`burrow status` lists every certificate with where it comes from, its state, when it expires and when it is renewed; a failing one says why. The admin UI shows the same on the Overview and lists problems under Needs attention. Check, in order:

1. **DNS:** the name points at this server (`dig +short blog.example.com`), for IPv6 too if you have an `AAAA` record.
2. **Ports:** port 443 (or 80) reaches burrow from the internet, not only from your network. A router must forward it.
3. **Firewalls:** nothing blocks Let's Encrypt's checks from abroad.
4. **Limits:** after many failures Let's Encrypt slows down; burrow follows its schedule. `directory: staging` tries everything against the test service without counting against your limits.

## Plain HTTP websites

```yaml
# server.yaml
web:
  http: 80
services:
  dashboard:
    client: home
    target: 3000
    web: dash.lan.example.com # no web.https: every site is plain HTTP
```

- With both ports, mark a site `tls: off` to serve it as plain HTTP.
- Anyone on the way can read and change plain-HTTP traffic. Keep such sites on networks you trust, or behind a proxy that does HTTPS. `burrow validate` marks every one as unencrypted, and so does the admin UI.

## Passthrough

With `tls: passthrough`, burrow decrypts nothing. It reads only the name at the start of the connection and passes every byte to the app, which keeps its own certificate. This suits devices that serve HTTPS already, like a NAS, a router or Proxmox, and TLS services that are not websites.

```yaml
# server.yaml
services:
  nas:
    client: home
    target: 192.168.1.20:443
    proxy_protocol: v2 # optional: the NAS sees the visitor's address
    web:
      hosts: [nas.example.com]
      tls: passthrough
```

On port 80, passthrough names get `404`: burrow cannot redirect a site whose HTTPS it does not do.

## Recipes

The everyday case: one site, its own certificate from Let's Encrypt:

```yaml
# server.yaml
web:
  https: 443
  http: 80
  letsencrypt:
    email: you@example.com
services:
  shop: { client: home, target: 8081, web: shop.example.com } # its own certificate (TLS or HTTP check)
```

A website with two names, with automatic HTTPS:

```yaml
# server.yaml
web:
  https: 443
  http: 80
  letsencrypt:
    email: ops@example.com
services:
  wordpress:
    client: home
    target: 8080
    web: [example.com, www.example.com]
```

An app that runs on the server itself:

```yaml
# server.yaml
services:
  git:
    client: self
    target: 3000
    web: git.example.com
```

A site for an IP address, with no domain:

```yaml
# server.yaml
web:
  https: 443
  http: 80
  letsencrypt:
    email: ops@example.com
services:
  kiosk:
    client: home
    target: 8080
    web: 203.0.113.10 # the server's public address
```

A dashboard on the home network, without certificates:

```yaml
# server.yaml
web:
  http: 80
services:
  dashboard:
    client: home
    target: 3000
    web: dash.lan.example.com
```

Your own wildcard certificate for many apps:

```yaml
# server.yaml
web:
  https: 443
  certificates:
    - hosts: ["*.apps.example.com"]
      cert: certs/apps.pem
      key: certs/apps.key
services:
  grafana:
    client: home
    target: 3000
    web: grafana.apps.example.com
  wiki:
    client: office
    target: 8090
    web: wiki.apps.example.com
```

## The tunnel on the same port

burrow clients can connect over `wss://` on the same port as the websites, to a name of their own:

```yaml
# server.yaml
web:
  https: 443
  letsencrypt:
    email: ops@example.com
websocket:
  listen: 443 # the same as web.https: shared
  host: tunnel.example.com
  path: /burrow
```

```yaml
# client.yaml
server: wss://tunnel.example.com/burrow
```

- The tunnel's name gets its certificate like a site: `websocket.tls` files, else a file in `web.certificates`, else Let's Encrypt. A `web:` block with no websites is enough for that.
- Write both ports the same way: `443` and `443`, or `0.0.0.0:443` and `0.0.0.0:443`.
- The tunnel can also use a website's name, taking only its `path` there, or share the plain port `web.http` over `ws://` (still encrypted by burrow). See [websocket.md](websocket.md#automatic-https-for-the-tunnel).

## What validate checks

| Problem | What validate says |
| --- | --- |
| a service has `web` but there is no top-level `web` | `services.blog.web: the web proxy is off; add a top-level web: with its port, like web: {https: 443}` |
| `web` has no port | `web needs a port: https for HTTPS sites, like https: 443, or http for plain HTTP sites, like http: 80` |
| a website with `tls: off` but no `web.http` | `services.wiki.web.tls: off serves plain HTTP, but web.http is not set; add http: 80` |
| `https_redirect: false` with no `web.http` | `services.blog.web.https_redirect: false serves plain HTTP too, but web.http is not set; add http: 80` |
| `https_redirect: false` on passthrough | `services.nas.web.https_redirect: false: burrow cannot serve plain HTTP for a passthrough service (the app keeps its own TLS); only the redirect is possible` |
| an HTTPS name with no certificate | `host "blog.example.com" (service "blog") has no certificate; add web.letsencrypt for automatic certificates, tls: {cert, key} on service "blog", or an entry in web.certificates whose hosts lists it` |
| Let's Encrypt cannot check a name | `blog.example.com (service "blog") cannot be checked by Let's Encrypt: open http: 80, or list a file for it in web.certificates (DNS checks come in a later phase)` |
| an entry in `web.certificates` without its files | `web.certificates[0] needs hosts, cert and key: hosts names which names use the file, cert and key are the files` |
| a wildcard from Let's Encrypt | `services.apps.web.hosts: "*.apps.example.com" needs DNS-01 (...); DNS checks come in a later phase, so add a certificate file under web.certificates or tls` |
| a name in two certificate entries | `host "files.example.com" is listed in web.certificates[0] and [2]; list it in one entry only` |
| a name that is not a host name | `services.blog.web.hosts: "https://blog.example.com" is not a host name; write the name alone, like blog.example.com` |
| a short name on an HTTPS site | `"nas" is a single-label name; ... use a full name like nas.home.example.com, or set tls: off` |
| a wildcard that is too wide | `services.apps.web.hosts: "*.com" is too wide; a wildcard needs a parent name, like *.example.com` |
| one name in two services | `host "nas.example.com" is used by services "nas" and "nas2"; each host name leads to one service` |
| the tunnel's name without a shared port | `websocket.host is only used when websocket.listen is the same port as web.https or web.http; ...` |
| the tunnel's name has no certificate | `websocket.host "tunnel.example.com" has no certificate; add web.letsencrypt, websocket.tls, or an entry in web.certificates whose hosts lists it` |
| a UDP service with `web` | `service "dns" is udp, but web only works for tcp services; remove web:, or set protocol: tcp` |
| warning: a low port burrow may not open | `web.https [::]:443: port 443 needs root or CAP_NET_BIND_SERVICE; burrow service install grants it, or run burrow as root` |
| another listener on the same port | `web.https [::]:443 is also the control port (listen ...)`, or the service or metrics port it clashes with |

## Limits

- A connection has 10 seconds to send its first TLS message, and at most 16 KiB of it is read before the name is known.
- Connections that have not shown a name yet are limited like the tunnel's own handshakes (`limits.max_handshakes`), with their own counter: a flood of web connections cannot block tunnel clients, and the other way round.
- A tunnel connection on the shared port has `limits.handshake_timeout_secs` in total for TLS, the WebSocket upgrade and burrow's own handshake.
- Stopping or reloading the server lets a request in flight finish for up to 10 seconds.
