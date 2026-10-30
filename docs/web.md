# Websites on one HTTPS port

burrow can serve your apps as websites. A visitor opens `https://blog.example.com`, the server receives the request on port 443, and burrow sends it through the tunnel to the app on your client. Many websites share one port, told apart by their names. burrow does HTTPS itself, with certificates from Let's Encrypt or your own files, or passes the encrypted connection to an app that does its own HTTPS.

```
browser ── https://blog.example.com ──> server:443 ══ tunnel ══ client ──> app on 127.0.0.1:8080
burrow client ── wss://tunnel.example.com/burrow ──> server:443 (the tunnel can share the port)
```

Use it to put a blog, a wiki, Home Assistant, Nextcloud or any web app at home on its own name with a valid certificate, with no nginx, Caddy or certbot on the server. Nothing runs unless you write it in the config: a top-level `web:` block starts the web proxy, and a service becomes a website with its own `web:`.

New words, like wildcard, passthrough or the Let's Encrypt checks, are in the [glossary](glossary.md#websites).

## A first website

This config serves the app on port 8080 of the home machine as `https://blog.example.com`, with a certificate from Let's Encrypt:

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

Then:

1. Point `blog.example.com` at the server in your DNS (an `A` or `AAAA` record).
2. Open ports 443 and 80 in the server's firewall.
3. Start the server, or save the config if it runs already.

burrow gets a certificate for `blog.example.com` from Let's Encrypt within a minute or so, and the site is up. Until then, a visitor on `http://` sees a short page saying the certificate is on its way. `burrow validate` shows what will happen:

```text
server.yaml: valid server config (1 client, 1 service)
web: https [::]:443 (HTTP/2 and HTTP/1.1, IPv4 and IPv6), http [::]:80 (IPv4 and IPv6)
certificates: Let's Encrypt (production) as ops@example.com
blog.example.com: Let's Encrypt, TLS check -> service blog
http [::]:80: HTTPS hosts redirect to https:// (308); Let's Encrypt HTTP checks answered
```

`burrow init server` can write this for you: answer yes to its website question, or pass `--web blog.example.com --email ops@example.com`.

## The ports

The web proxy listens on up to two ports of the server, set under `web:`:

| Key | What it does |
| --- | --- |
| `web.https` | HTTPS for every website, and the TLS services passed through by name. |
| `web.http` | Plain HTTP: it sends `http://` visitors to `https://`, serves plain-HTTP websites, and answers Let's Encrypt's HTTP check. |

- A port alone, like `443`, listens on IPv4 and IPv6, since many home connections only have a public IPv6 address. Write `0.0.0.0:443` for IPv4 only, or one address, like `203.0.113.10:443`.
- `web:` needs at least one of the two. Without `web.https`, every website is plain HTTP.
- Ports below 1024 need root or `CAP_NET_BIND_SERVICE` on Linux; `burrow service install` takes care of that.
- Without `web.http`, nothing answers `http://`: a visitor who types `http://blog.example.com` gets "connection refused".

## How a website is served

A service becomes a website with its own `web:`. The short forms `web: blog.example.com` and `web: [example.com, www.example.com]` list the names. The long form says more; this one serves a site on two names, with the defaults written out:

```yaml
# server.yaml
web:
  https: 443
  http: 80
  letsencrypt:
    email: ops@example.com
services:
  blog:
    client: home
    target: 8080
    web:
      hosts: [example.com, www.example.com]
      tls: auto # the default when web.https is set
      https_redirect: true # the default: http:// goes to https://
```

`tls` says who does HTTPS for the site, and where its certificate comes from:

| `tls` | What happens |
| --- | --- |
| `auto` | burrow does HTTPS with a certificate for every name: from a file in `web.certificates` that lists the name, else from Let's Encrypt. The default with `web.https`. |
| `{cert: ..., key: ...}` | burrow does HTTPS with this site's own certificate file, which must cover every name of the site; `{upload: ...}` is one uploaded in the admin UI. |
| `internal` | burrow does HTTPS with certificates from its own local CA, for names on your network like `nas.lan`; install the CA on your devices once ([The local CA](web/certificates.md#the-local-ca-tls-internal)). |
| `passthrough` | The app does its own HTTPS. burrow only reads the name and passes the encrypted bytes on. |
| `off` | No HTTPS: the site is plain HTTP on `web.http`. The default without `web.https`. |

`https_redirect` decides what an HTTPS site does on `web.http`. `true`, the default, sends visitors to the same address on `https://` with a permanent redirect (308); `false` answers plain HTTP too. Set it for every site under `web:`, or per site.

The names of a site follow these rules:

- `hosts` takes DNS names, like `blog.example.com`, wildcards of one label, like `*.apps.example.com`, and IP addresses, like `203.0.113.10`.
- A wildcard covers exactly one label: `*.apps.example.com` covers `x.apps.example.com`, not `apps.example.com` and not `a.b.apps.example.com`. An exact name in another service wins over a wildcard.
- Each name leads to one service. Upper and lower case do not matter, and a final dot is ignored.
- A name without a dot, like `nas`, is allowed only on a plain-HTTP site (`tls: off`). HTTPS needs a full name, like `nas.home.example.com`. The name a browser sends in TLS (SNI) is a fully qualified DNS name (RFC 6066 section 3), and public CAs do not issue certificates for single-label names.
- Names are added and removed when you save the config.

### What burrow does for a website

Every website gets these without any setting:

- **Requests go to the app through the tunnel**, one connection per visitor connection. WebSocket upgrades work.
- **The app learns who asked:** burrow sets `X-Forwarded-For`, `X-Forwarded-Proto`, `X-Forwarded-Host`, `X-Forwarded-Port` and `X-Real-IP`. A visitor's own `Forwarded` and `X-Forwarded-*` headers are removed first; only the proxies in `web.trusted_proxies` are believed ([Behind Cloudflare or a load balancer](web/proxies.md#behind-cloudflare-or-a-load-balancer)).
- **The name must match:** a request whose `Host` differs from the name of the HTTPS connection gets `421`, and a request without `Host` gets `400`.
- **Pages when something is wrong**, instead of a broken connection:
  - `503` when no client of the service is connected, all are down, or the service is at its connection limit;
  - `502` when the client cannot reach the app;
  - `404` for an unknown name on port 80.

  A failure after the app started answering closes the connection.
- **Time limits:**
  - A visitor has 10 seconds to send a request's headers (`web.request_header_timeout_secs`). This is also how long an idle kept-alive connection stays open.
  - A connection that moves no data for 60 seconds is closed (`web.idle_timeout_secs`); WebSockets stay open.
  - A connection waiting for the app's next bytes is not idle. An app that goes quiet in the middle of an answer, like a quiet event stream, is kept open while the visitor stays, and closed when the visitor leaves.
  - A visitor who stops reading an answer counts as idle.
- **TLS 1.2 and 1.3.** TLS 1.0 and 1.1 are retired (RFC 8996). Clients that only speak them cannot reach any site on the port, passthrough sites included.
- Everything else a service does works for websites too: several clients with failover, balance or least_connections, health checks, `allow_from`, `max_connections`, `bandwidth`.
- **`allow_from` limits visitors, not Let's Encrypt:** burrow answers Let's Encrypt's checks before it looks at the site, so a site open only to `192.168.1.0/24` still gets its certificate. Behind a CDN or another proxy, `allow_from` sees the proxy's address, not the visitor's ([Behind a proxy](web/proxies.md)).

## Plain HTTP websites

A site on your home network, or behind a proxy that already does HTTPS, may not need HTTPS from burrow. Without `web.https`, every site is plain HTTP. This one serves a dashboard to the home network only:

```yaml
# server.yaml
web:
  http: 80
services:
  dashboard:
    client: home
    target: 3000
    allow_from: [192.168.1.0/24] # only the home network
    web: dash.lan.example.com # no web.https: every site is plain HTTP
```

`allow_from` keeps the site to your network only when the burrow server is on that network. A server on the internet sees your router's public address: there, use the router's address or a VPN range instead.

- With both ports, mark a site `tls: off` to serve it as plain HTTP.
- Anyone on the way can read and change plain-HTTP traffic. Keep such sites on networks you trust, or behind a proxy that does HTTPS. `burrow validate` marks every one as unencrypted, and so does the admin UI.

## Passthrough

With `tls: passthrough`, burrow decrypts nothing. It reads only the name at the start of the connection and passes every byte to the app, which keeps its own certificate. This suits devices that already serve HTTPS, like a NAS, a router or Proxmox, and TLS services that are not websites.

```yaml
# server.yaml
web:
  https: 443 # no certificate needed: the NAS keeps its own
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

## More about websites

Each page below covers one part in depth, and links back to the step of [How it works](web/how-it-works.md) it changes.

| Page | What it covers |
| --- | --- |
| [How it works](web/how-it-works.md) | what burrow does with a connection and a request, step by step, in the order the code runs |
| [Certificates](web/certificates.md) | your own files, Let's Encrypt, DNS checks and shared wildcards, when a certificate fails |
| [Paths and redirects](web/routing.md) | several apps on one name, how a path is read, redirects |
| [Headers, limits and pages](web/headers-limits-pages.md) | header rules, upload limits, timeouts, error pages, HTTPS to the app |
| [Who may visit](web/access.md) | addresses, rate limits, passwords, single sign-on with Authelia or authentik, device certificates |
| [Behind a proxy, a load balancer or a home router](web/proxies.md) | Cloudflare, cloudflared, PROXY headers, port forwarding at home |
| [HTTP/2 and gRPC](web/http2-grpc.md) | HTTP/2 to browsers, gRPC apps and their streams |
| [Health checks for websites](web/health-checks.md) | checking an app with a path |
| [Access log and request ids](web/logs.md) | one line per request, request ids |
| [The tunnel on the web ports](web/tunnel.md) | burrow clients over wss:// on 443 or ws:// on 80 |
| [Recipes](web/recipes.md) | short configs for common apps |
| [A full example](web/full-example.md) | everything in one server.yaml, and what validate checks |
