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
http [::]:80: HTTPS hosts redirect to https:// (308); Let's Encrypt HTTP checks answered
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
- A name without a dot, like `nas`, is allowed only on a plain-HTTP site (`tls: off`). HTTPS needs a full name, like `nas.home.example.com`: the name a browser sends in TLS (SNI) is a fully qualified DNS name (RFC 6066 section 3), and public CAs do not issue certificates for single-label names.
- Names are added and removed when you save the config.

### What burrow does for a website

- **Requests go to the app through the tunnel**, one connection per visitor connection. WebSocket upgrades work.
- **The app learns who asked:** burrow sets `X-Forwarded-For`, `X-Forwarded-Proto`, `X-Forwarded-Host`, `X-Forwarded-Port` and `X-Real-IP`. A visitor's own `Forwarded` and `X-Forwarded-*` headers are removed first; only the proxies in `web.trusted_proxies` are believed ([Behind Cloudflare or a load balancer](#behind-cloudflare-or-a-load-balancer)).
- **The name must match:** a request whose `Host` differs from the name of the HTTPS connection gets `421`, and a request without `Host` gets `400`.
- **Pages when something is wrong**, instead of a broken connection: `503` when no client of the service is connected, all are down or the service is at its connection limit, `502` when the client cannot reach the app, `404` for an unknown name on port 80. A failure after the app started answering closes the connection.
- **Time limits:** a visitor has 10 seconds to send a request's headers (`web.request_header_timeout_secs`, also how long an idle kept-alive connection stays open), and a connection that moves no data for 60 seconds is closed (`web.idle_timeout_secs`; WebSockets stay open).
- **TLS 1.2 and 1.3.** TLS 1.0 and 1.1 are retired (RFC 8996); clients that only speak them cannot reach any site on the port, passthrough sites included.
- Everything else a service does works for websites too: several clients with failover or balance, health checks, `allow_from`, `max_connections`, `bandwidth`.
- **`allow_from` limits visitors, not Let's Encrypt:** burrow answers Let's Encrypt's checks before it looks at the site, so a site open only to `192.168.1.0/24` still gets its certificate. Behind a CDN or another proxy, `allow_from` sees the proxy's address, not the visitor's.

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
- `burrow validate` (and every start and reload) reads the files: it refuses one it cannot read, one that is not PEM, a key that does not fit its certificate, and a certificate that does not cover a name it is used for.
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
- **How Let's Encrypt checks a name:** over TLS on port 443 when `web.https` is 443, else over HTTP on port 80 when `web.http` is 80. Each name needs one of them reachable from the internet, also from several places, so a firewall that allows only some countries or addresses breaks the check. A site's `challenge: tls`, `challenge: http` or `challenge: dns` picks the check (`websocket.challenge` for the tunnel's name); validate refuses a check whose port is not there, `http` or `tls` for a wildcard, and `dns` for an IP address.
- **Wildcards** need a DNS check (below), or your own file.
- **Many sites under one domain** are best served by one wildcard certificate, like `*.example.com`: from Let's Encrypt as a shared wildcard on its DNS provider (below), or as your own file in `web.certificates`.
- **IP addresses** get Let's Encrypt's short-lived certificates (about 6 days), renewed after half their life.
- **`profile`** applies to certificates for DNS names; IP addresses always get the short-lived profile (or none if the CA doesn't list it). burrow checks the profile against the list the CA offers before ordering; one it does not offer is shown in `burrow status` and the admin UI with the list of the offered ones.
- **Renewal** happens by itself, at the time Let's Encrypt suggests (ARI), else after two thirds of a certificate's life. A failed try is retried after 1 minute, 10 minutes, 100 minutes, then daily, and the old certificate keeps being served, even after it expires.
- **No duplicates:** certificates are kept by their names. A restart, a renamed service or a name moved to another service orders nothing new, and at most 4 new certificates per set of names are ordered in 7 days.
- **Where they are kept:** a private folder that `burrow status` names. An installed service keeps it in `/var/lib/burrow/<name>` on Linux, `/Library/Application Support/burrow/<name>` on macOS, `%ProgramData%\burrow\services\<name>` on Windows; `burrow service uninstall` keeps it and only `--purge` deletes it. A server run by hand uses one folder per config file; `burrow server --state-dir DIR` picks another.
- **The account** is shown in `burrow status` and `burrow validate` (once the first start has created it), for a CAA DNS record with `accounturi` that lets only this account get certificates for your domain.

### DNS checks: wildcards and names the internet cannot reach

With a DNS check, Let's Encrypt reads a TXT record that burrow sets at `_acme-challenge.<name>`. No port has to be reachable from the internet, so it suits wildcards (Let's Encrypt gives them only this way), names inside your LAN, and servers whose ports 80 and 443 are closed. `letsencrypt.dns` lists how burrow sets records, each provider with the domains it manages:

```yaml
# server.yaml
web:
  https: 443
  letsencrypt:
    email: ops@example.com
    dns:
      - domains: [example.com, example.net] # these domains and every name under them
        cloudflare: { token: "${CF_TOKEN}" } # quoted: ${...} inside { } must be a quoted string in YAML
        wildcards: ["*.example.com"] # one shared certificate: example.com and *.example.com
      - domains: [corp.example.org]
        rfc2136: { server: "192.168.1.2:53", key_name: burrow-key, secret: "${TSIG_SECRET}" }
services:
  wiki:
    client: home
    target: 3001
    web: wiki.example.com # covered by *.example.com: orders nothing of its own
  erp:
    client: office
    target: 8443
    web:
      hosts: [erp.corp.example.org]
      challenge: dns # only reachable inside the LAN
```

- **Which entry a name uses:** the one whose domain covers it most specifically, so `corp.example.org` wins over `example.org`. A domain covers itself and every name under it; write `example.com`, not `*.example.com`. Each domain sits in one entry only.
- **Which names use DNS:** wildcards always, and a site with `challenge: dns` (the tunnel's own name with `websocket.challenge: dns`). Other names keep the TLS or HTTP check.
- **The record must be public.** The name may be internal, but Let's Encrypt reads the TXT record from the internet, so the zone's public nameservers must answer it. An `rfc2136` server on your LAN works as the zone's hidden primary that feeds the public nameservers; a split-horizon server that answers only inside the LAN cannot work.
- **burrow waits for every nameserver.** Before it tells Let's Encrypt to look, burrow asks each of the zone's public nameservers directly until all of them answer the record, for up to `wait_secs` (120 when not set). `resolvers` (like `[1.1.1.1, 9.9.9.9]`) are used only to find those nameservers, and must give the public view; without it, the system's resolvers are used.
- **CNAMEs are followed.** If `_acme-challenge.<name>` is a CNAME, burrow writes the record at its target (up to 8 steps), as Let's Encrypt reads it there. The entry is still chosen by the name's own domain: to delegate, list the name's domain on the entry that can write the target.
- **Records are removed** after each order, also when it fails.
- **Secrets** (`token`, `secret`, `password`) come only from `${VAR}` or `file:path` (relative to the config file); validate refuses one written in the file. They are never printed: `burrow status`, the logs and the admin UI show only that they are set.

#### Shared wildcards

Many sites under one domain are best served by one wildcard certificate: one order and one renewal instead of one per site. List it in `wildcards` on the entry that manages its domain, like `wildcards: ["*.example.com"]` above.

- Each wildcard is one certificate that also covers its parent name (`*.example.com` with `example.com`).
- It is ordered when the config is saved, kept, and renewed by itself. Every site one label under it reuses it, also one added later; names two labels down get their own.
- Removing it from the list stops its renewals.
- burrow never makes a wildcard by itself. When three or more sites under one DNS-covered domain would each get their own certificate, validate suggests one.

#### Cloudflare

Create an API token at <https://dash.cloudflare.com/profile/api-tokens> with **Zone: Read** and **DNS: Edit**, limited to the zones you list (Zone Resources: Include, Specific zone). Give it to burrow as `${CF_TOKEN}` from the service's environment, or `file:/etc/burrow/cf.token`.

#### RFC 2136: BIND, Knot, PowerDNS, Technitium

burrow sends a dynamic update signed with a TSIG key (hmac-sha256) to `server`, the zone's primary, and finds the zone itself. Make a key, and allow it to change only the `_acme-challenge` TXT records:

```text
# BIND: tsig-keygen -a hmac-sha256 burrow-key > /etc/bind/burrow-key.conf, then in the zone:
zone "example.org" {
    type primary;
    file "/var/lib/bind/example.org.db";
    update-policy { grant burrow-key name _acme-challenge.erp.corp.example.org. TXT; };
};
```

- **Knot DNS:** a `key` with `algorithm: hmac-sha256`, and an `acl` with `action: update`, `update-type: [TXT]` and `update-owner: name` for the `_acme-challenge` names.
- **PowerDNS:** set `dnsupdate=yes` in `pdns.conf`, make the key with `pdnsutil tsigkey generate burrow-key hmac-sha256` (before 5.0: `pdnsutil generate-tsig-key`), and allow it on the zone with `pdnsutil metadata add example.org TSIG-ALLOW-DNSUPDATE burrow-key`. Limiting the key to `_acme-challenge` names needs PowerDNS's Lua update policy.
- **Technitium:** allow dynamic updates for the zone with a TSIG key in the zone's options, as Technitium's documentation describes, limited to the `_acme-challenge` TXT records where it offers that.

`secret` is the key's base64 secret, as in the key file.

#### acme-dns

[acme-dns](https://github.com/acme-dns/acme-dns) serves only ACME records, so your main DNS provider needs no API at all: point each name's `_acme-challenge` CNAME at the acme-dns server once, and burrow updates the record there.

```yaml
# server.yaml
web:
  https: 443
  letsencrypt:
    email: ops@example.com
    dns:
      - domains: [example.io]
        acme_dns:
          api: https://auth.example.org
          username: ${ACMEDNS_USER}
          password: ${ACMEDNS_KEY}
          subdomain: d420c923-bbd7-4056-ab64-c3ca54c9b3cf
```

#### A command for any provider

`command` runs a program of yours, like lego's `exec` provider: `PROGRAM present|cleanup NAME VALUE`, where `NAME` is the record's name with a final dot (the `_acme-challenge` name, or its CNAME target). It gets no secrets from burrow and an empty environment apart from `PATH`, so it reads its own credentials from a file. It must finish within 60 seconds, and it must add the value, not replace the record: a wildcard and its parent name set two values side by side. Installed as a service, it runs as the service account under the service's limits: on Linux it cannot see /home, can write only burrow's own folders, and cannot run programs that compile code at run time (`MemoryDenyWriteExecute`, so a Node.js hook fails); keep it and its credentials file outside /home, readable by that account.

```sh
#!/bin/sh
# /usr/local/bin/burrow-dns: $1 = present|cleanup, $2 = _acme-challenge.NAME. , $3 = value
case "$1" in
  present) nsupdate -k /etc/burrow/tsig.key <<EOF
update add $2 60 TXT "$3"
send
EOF
  ;;
  cleanup) nsupdate -k /etc/burrow/tsig.key <<EOF
update delete $2 TXT "$3"
send
EOF
  ;;
esac
```

Scripts written for lego's exec provider work unchanged when they read their credentials themselves.

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
    allow_from: [192.168.1.0/24] # only the home network
    web: dash.lan.example.com # no web.https: every site is plain HTTP
```

`allow_from` keeps the site to your network only when the burrow server is on that network; a server on the internet sees your router's public address, so there use the router's address or a VPN range instead.

- With both ports, mark a site `tls: off` to serve it as plain HTTP.
- Anyone on the way can read and change plain-HTTP traffic. Keep such sites on networks you trust, or behind a proxy that does HTTPS. `burrow validate` marks every one as unencrypted, and so does the admin UI.

## Passthrough

With `tls: passthrough`, burrow decrypts nothing. It reads only the name at the start of the connection and passes every byte to the app, which keeps its own certificate. This suits devices that serve HTTPS already, like a NAS, a router or Proxmox, and TLS services that are not websites.

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

## Paths: several apps on one name

One name can be split between services by `paths`. The longest matching path wins, and a path covers itself and everything below it: `/api` matches `/api` and `/api/users`, never `/apix`. A service without `paths` takes everything the others leave.

```yaml
# server.yaml
web:
  https: 443
  http: 80
  letsencrypt:
    email: ops@example.com
services:
  site: # the website runs at home
    client: home
    target: 8080
    web: example.com # everything on example.com not claimed below
  api: # the API runs in the office
    client: office
    target: 9000
    web:
      hosts: [example.com]
      paths: [/api]
      app_path: / # the API app has no /api prefix of its own
```

`example.com/` goes to the home client's port 8080, and `example.com/api/users` to the office client's port 9000 as `/users`. One certificate covers `example.com` for both. If the office client is down, `/api` answers 503 while the rest of the site works.

- **`app_path`** is what the app sees in place of the matched path: `paths: [/api]` with `app_path: /v1` sends `/api/users` as `/v1/users`, and `app_path: /` removes the prefix. When the path changes, the app gets `X-Forwarded-Prefix: /api`, its public prefix; an app that builds absolute links must read it, or be told its public address, as behind any proxy that rewrites paths.
- **Routing is not a redirect.** Routing forwards the request and the address bar stays the same; a redirect sends the browser to another address. `paths` and `app_path` route; `redirect_from`, `redirects` and `web.redirect_names` redirect.
- **Two services may share a name only by different paths**, and they must choose the same `tls`.

### How burrow reads a path

burrow cleans each request's path once, before it picks a route, and sends the clean path to the app. Paths are matched like nginx, and forwarded cleaned like Traefik:

- repeated slashes are merged: `//admin` is `/admin`;
- escapes of plain characters are decoded and the others written in capitals: `/%61pi` is `/api`, and `%2f` is `%2F`;
- a `.` or `..` segment is refused with `400`, also when it is encoded (`%2E%2E`) or hides behind an encoded slash (`/public%2F..%2Fadmin`, `/a%5C..%5Cb`);
- an encoded slash on its own is data and passes unchanged, like GitLab's `/api/v4/projects/group%2Fproject`.

So a password on `/wp-admin` cannot be walked around with `//wp-admin` or `/%77p-admin`. The paths you write in the config are cleaned the same way.

## Redirects

```yaml
# server.yaml
web:
  https: 443
  http: 80 # without it, http:// gets no answer at all, not even a redirect
  letsencrypt:
    email: ops@example.com
  redirect_names: # whole names with no site of their own
    old.example.com: "https://example.com" # path and query kept: /a?b -> https://example.com/a?b
    example.com/shop: { to: "https://shop.example.net/", permanent: true } # /shop/cart -> https://shop.example.net/cart
services:
  blog:
    client: home
    target: 8080
    web:
      hosts: [example.com]
      redirect_from: [www.example.com] # permanent (308) to example.com, path and query kept
      redirects:
        /old: /new # temporary (307): /old/page -> /new/page
        /feed: { to: "https://feeds.example.net/blog", permanent: true }
```

- `redirect_from` sends other names to the site's first name, for good (308).
- `redirects` sends paths of the site elsewhere: temporary (307) unless `permanent: true` (308). The rest of the path and the query are kept, and they are matched on the public path, before `app_path`. Browsers remember a permanent redirect, so use it only when you are sure.
- `web.redirect_names` is for names with no site of their own, or one path of a name another service serves (`example.com/shop` above): a longer path is a route of its own.
- A target is a full address (`https://...`) or a path on the same name (`/new`).
- Names that only redirect still get a certificate, the same way as a site's names.

## Headers, limits and pages

These keys work under `web:` for every site, and in a service's `web:` for that site; the site's own value wins.

```yaml
# server.yaml
web:
  https: 443
  http: 80
  letsencrypt:
    email: ops@example.com
  hsts: true # Strict-Transport-Security for a year, on HTTPS answers only
  compress: true # gzip text answers when the browser asks for it
  max_body_size: 100MB # bigger uploads get 413
  response_header_timeout_secs: 60 # the app's time to start its answer, 504 after
  response_headers:
    X-Content-Type-Options: nosniff
    Server: "" # empty: remove the header
  error_pages: pages/ # any of 404.html, 413.html, 429.html, 502.html, 503.html, 504.html, maintenance.html
services:
  pve: # Proxmox: HTTPS with its own self-made certificate
    client: home
    target: 192.168.1.10:8006
    web:
      hosts: [pve.example.com]
      target_tls: { insecure_skip_verify: true }
      max_body_size: 8GB # ISO uploads
  shop:
    client: office
    target: 8081
    web:
      hosts: [shop.example.com]
      maintenance: true # every visitor sees the maintenance page (503 with Retry-After)
```

- **`request_headers` and `response_headers`** set headers on requests to the app and on answers to visitors, `Name: value`; an empty value removes one. `Host` may be set for an app that wants its own name; `X-Forwarded-Host` keeps the visitor's. Headers of HTTP's own framing, like `Connection` or `Transfer-Encoding`, cannot be set.
- **`max_body_size`** has no limit by default: bodies are streamed to the app, never held in memory. A larger `Content-Length` gets 413 before the app is asked, without a `100 Continue`.
- **`error_pages`** replaces burrow's built-in pages with yours; a page that is not in the folder stays built in. The folder is read when the config is saved.
- **`hsts`** is never sent on port 80.
- **`compress`** gzips HTML, CSS, JavaScript, JSON, XML, SVG and wasm answers of 1 KiB or more (or of unknown length), when the browser accepts gzip and the app did not compress or mark them `no-transform` already.

### HTTPS to the app

`target_tls` is for apps that speak HTTPS themselves:

| `target_tls` | The app's certificate is |
| --- | --- |
| `true` | checked against the public CAs, for the name or IP address of the target |
| `{ca: certs/corp-ca.pem}` | checked against your CA only |
| `{insecure_skip_verify: true}` | not checked, for apps with a self-made certificate like Proxmox, UniFi or DSM |

An IP target needs its IP address in the app's certificate. A plain-HTTP app behind `target_tls` answers with a clear 502.

## Behind Cloudflare or a load balancer

```yaml
# server.yaml
web:
  https: 443
  letsencrypt:
    email: ops@example.com
  proxy_protocol_from: [10.0.0.5/32] # a TCP load balancer: it must send a PROXY header
  trusted_proxies: [173.245.48.0/20, 103.21.244.0/22] # Cloudflare's ranges
```

- **`trusted_proxies`** lists the HTTP proxies in front of burrow whose `X-Forwarded-For` and `X-Forwarded-Proto` are believed. burrow reads `X-Forwarded-For` from the right and takes the first address that is not a trusted proxy. From anyone else, these headers, `Forwarded` and every other `X-Forwarded-*` (also written with `_`) are removed before burrow sets its own. Cloudflare publishes its ranges at <https://www.cloudflare.com/ips/>; list them all.
- **Behind Cloudflare Tunnel (`cloudflared`)**, the proxy in front of burrow is `cloudflared` itself: `trusted_proxies` is its address, for example `127.0.0.1/32`, not Cloudflare's ranges.
- **`proxy_protocol_from`** lists TCP load balancers that start each connection with a PROXY header (v1 or v2). They must send it, and nobody else may. Their visitor's address is used from then on.
- The reported address is what `allow_from`, the access log and a service's own `proxy_protocol` see.
- **Let's Encrypt behind a proxy that does HTTPS:** the TLS check cannot pass through it. Use `challenge: http` (with `http: 80`) or `challenge: dns` on such sites; validate notes the sites that would use the TLS check.
- **A plain-HTTP site behind such a proxy** can still send `http://` visitors to `https://`: `https_redirect: true` on a `tls: off` site with `trusted_proxies` redirects by the proxy's `X-Forwarded-Proto`. If the proxy does not send it, every request redirects.

## Access log and request ids

```yaml
# server.yaml
web:
  https: 443
  letsencrypt:
    email: ops@example.com
  access_log:
    file: logs/access.log
    format: json # or common (the default, like nginx and Apache)
  request_id: true # an X-Request-Id for each request
```

- One line per request: time, client address, name, method, path, protocol, status, bytes, time taken, service and request id. `access_log: logs/access.log` is the short form.
- `request_id` gives each request an `X-Request-Id`: sent to the app, returned to the visitor and written to the log. One sent by a trusted proxy is kept; a visitor's own is replaced.

## Examples

Each example is the web part of a `server.yaml`: add it to the `listen:` and `clients:` of [A first website](#a-first-website) (`office` is a second client, written like `home`). Every one of them is checked by burrow's tests.

### The minimum: websites with automatic HTTPS

```yaml
# server.yaml
web:
  https: 443
  http: 80
  letsencrypt:
    email: you@example.com
services:
  shop:
    client: home
    target: 8081
    web: shop.example.com
```

### One site with two names

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
    web: [example.com, www.example.com] # one certificate for both names
```

A service's names get one certificate together: `example.com` and `www.example.com` above share one, and adding or removing a name orders a new one for the new set.

### Your own certificates only (Let's Encrypt never contacted)

```yaml
# server.yaml
web:
  https: 443
  certificates:
    - hosts: ["*.example.com"]
      cert: certs/wildcard.example.com.pem
      key: certs/wildcard.example.com.key
services:
  shop:
    client: home
    target: 8081
    web: shop.example.com
```

### Many sites under one domain: one shared wildcard

```yaml
# server.yaml
web:
  https: 443
  letsencrypt:
    email: you@example.com
    dns:
      - domains: [example.com]
        cloudflare: { token: "${CF_TOKEN}" }
        wildcards: ["*.example.com"] # one certificate for every site below
services:
  wiki: { client: home, target: 3001, web: wiki.example.com }
  blog: { client: home, target: 8080, web: blog.example.com }
  auth: { client: self, target: 9091, web: auth.example.com }
```

### Many apps under your own wildcard file

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

### Plain HTTP only (LAN, or behind a proxy that does HTTPS)

```yaml
# server.yaml
web:
  http: 80 # no https port: every site is plain http, no certificates
services:
  dashboard:
    client: home
    target: 8095
    allow_from: [192.168.1.0/24] # only devices on the home network
    web: dash.lan.example.com
```

`allow_from` sees the address the server sees. It keeps the site to your network only when the burrow server is on that network too; a server on the internet sees your router's public address instead.

### An app on the server itself

```yaml
# server.yaml
web:
  https: 443
  http: 80
  letsencrypt:
    email: ops@example.com
services:
  git:
    client: self # the server is its own client
    target: 3000
    web: git.example.com
```

`client: self` reaches its own target without a `self:` block; `self.allow` only limits what visitors may `reach` on the server ([services.md](services.md)).

### Home Assistant

```yaml
# server.yaml
web:
  https: 443
  http: 80
  letsencrypt:
    email: ops@example.com
services:
  homeassistant:
    client: home
    target: 127.0.0.1:8123 # Home Assistant Container; Home Assistant OS uses port 80 since 2026.8
    web: ha.example.com
```

Home Assistant's pages and apps keep a WebSocket open (`/api/websocket`), which burrow passes through like any other request. Home Assistant blocks requests that come through a reverse proxy it does not trust, so in Home Assistant open Settings > System > Network, and under HTTP server turn on Trust X-Forwarded-For and add the address of the machine that runs the burrow client to Trusted proxies (`127.0.0.1` when the client runs on the Home Assistant machine), as its [HTTP integration docs](https://www.home-assistant.io/integrations/http/) say.

### Nextcloud behind Cloudflare

```yaml
# server.yaml
web:
  https: 443
  http: 80
  letsencrypt:
    email: you@example.com
  trusted_proxies: [173.245.48.0/20, 103.21.244.0/22] # Cloudflare's ranges (all of https://www.cloudflare.com/ips/)
services:
  cloud:
    client: home
    target: 127.0.0.1:8080
    web:
      hosts: [cloud.example.com]
      challenge: http # Cloudflare's proxy does the HTTPS, so the TLS check cannot pass through it
      max_body_size: 16GB # big uploads
```

In Nextcloud's `config.php`, trust the address the burrow client connects from (`'trusted_proxies' => ['127.0.0.1']` when it runs on the same machine), add `cloud.example.com` to `trusted_domains`, and set `'overwriteprotocol' => 'https'`. Nextcloud then reads the visitor's address from `X-Forwarded-For`, which burrow fills from Cloudflare's.

### Grafana under a path

```yaml
# server.yaml
web:
  https: 443
  letsencrypt:
    email: you@example.com
services:
  site:
    client: home
    target: 8080
    web: example.com
  grafana:
    client: home
    target: 3000
    web:
      hosts: [example.com]
      paths: [/grafana]
      app_path: / # Grafana gets /dashboards for /grafana/dashboards
```

Tell Grafana its public address so its links keep the prefix: `root_url = https://example.com/grafana/` in the `[server]` section of `grafana.ini`. Without `app_path`, set `serve_from_sub_path = true` there as well, and Grafana handles the prefix itself.

### A site for an IP address, with no domain

```yaml
# server.yaml
web:
  https: 443
  letsencrypt:
    email: ops@example.com
services:
  kiosk:
    client: home
    target: 8080
    web: 203.0.113.10 # the server's public address; a short-lived certificate
```

### On the client

A website's target is reached by its client, like any service. The client's `allow` list must permit it, and `burrow client-config home --server tunnel.example.com` prints a `client.yaml` that does:

```yaml
# client.yaml
server: tunnel.example.com:7835 # the server's listen: port, where clients connect
name: home
secret: ${HOME_SECRET}
allow:
  - 127.0.0.1/32:8081 # the shop app on this machine
```

`listen: 7835` on the server is the tunnel's own port, for clients and visitors; it is not a website port. Clients can also come in on 443, see below.

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

### The tunnel on port 8443, no port 80, through DNS

```yaml
# server.yaml
web:
  https: 8443
  letsencrypt:
    email: you@example.com
    dns:
      - domains: [example.com]
        cloudflare: { token: "${CF_TOKEN}" }
websocket:
  listen: 8443
  host: tunnel.example.com
  path: /burrow
  challenge: dns # dns | http | tls; clients use wss://tunnel.example.com:8443/burrow
```

### The tunnel on port 80, plain ws (still encrypted by burrow)

```yaml
# server.yaml
web:
  http: 80
websocket:
  listen: 80
  host: tunnel.example.com
  path: /burrow # clients use ws://tunnel.example.com/burrow
```

## A full example

Everything above in one `server.yaml`: the tunnel, the web settings and many kinds of website.

```yaml
# server.yaml
# ─────────────────────────────────────────────────────────────
# 1. THE TUNNEL
# ─────────────────────────────────────────────────────────────
listen: 7835 # clients and visitors connect here over TCP
public_address: tunnel.example.com

websocket: # clients can also connect over wss://
  listen: 443 # same port as the websites: shared, told apart by name
  host: tunnel.example.com # this name is the tunnel; it uses the *.example.com wildcard below
  path: /burrow

clients: # the machines that run your apps
  home:
    secret: ${HOME_SECRET}
  office:
    secret: ${OFFICE_SECRET}

visitors: # people who reach private services with "burrow visit"
  laptop:
    secret: ${LAPTOP_SECRET}

# ─────────────────────────────────────────────────────────────
# 2. WEBSITES: settings for all sites
#    Without this "web:" block, burrow serves no websites.
# ─────────────────────────────────────────────────────────────
web:
  https: 443 # secure sites. Leave it out and every site is plain http
  http: 80 # normal http://: redirects to https, plain sites, Let's Encrypt's HTTP check
  https_redirect: true # secure sites send http:// visitors to https:// (the default)

  letsencrypt: # free automatic certificates; writing this accepts Let's Encrypt's terms
    email: you@example.com
    dns: # DNS providers, for wildcards and names the internet can't reach
      - domains: [example.com, example.net] # Cloudflare manages these and every name under them
        cloudflare: { token: "${CF_TOKEN}" } # quoted: ${...} inside { } must be; a token for DNS of these domains only
        wildcards: ["*.example.com", "*.apps.example.com"] # shared certificates, got when you save
      - domains: [corp.example.org] # another domain, at your company's DNS server
        # 10.0.0.53 is the zone's primary; it must feed the public nameservers of corp.example.org,
        # because Let's Encrypt reads the record from the internet (only the name may be internal)
        rfc2136: { server: "10.0.0.53:53", key_name: acme, secret: "${TSIG_SECRET}" }

  certificates: # your own certificate files (hosts, cert and key all required)
    - hosts: ["*.intranet.example.org"]
      cert: certs/intranet-wildcard.pem
      key: certs/intranet-wildcard.key

  trusted_proxies: [173.245.48.0/20] # Cloudflare in front: believe its "real visitor" headers

  redirect_names: # old names with no site anymore: they only send visitors away
    old-blog.example.com: "https://example.com" # temporary
    example.io: { to: "https://example.com", permanent: true } # permanent: browsers remember it

  access_log: logs/access.log # one line per request (who, what, how long)
  request_id: true # an id per request, in the log and sent to the app

  # Defaults for every site (each site can change them)
  max_body_size: 100MB # biggest upload (no limit if you leave it out)
  compress: true # smaller text pages, faster loading
  hsts: true # browsers always use https for these sites (hard to undo!)
  request_header_timeout_secs: 10 # a visitor must send its request within 10 s
  response_header_timeout_secs: 60 # the app must start answering within 60 s (504 after)
  error_pages: pages/ # your own 404.html, 413.html, 429.html, 502.html, 503.html, 504.html, maintenance.html

# ─────────────────────────────────────────────────────────────
# 3. SERVICES: a service with "web:" is a website
#    tls: auto (default) | passthrough | off | { cert, key }
#    challenge: dns | http | tls (optional; automatic when not set)
# ─────────────────────────────────────────────────────────────
services:

  # The simplest website: covered by the *.example.com wildcard, so it orders nothing
  shop:
    client: home
    target: 8081
    web: shop.example.com

  # The main site; "www." sends visitors to the bare name
  blog:
    client: home
    target: 8080
    web:
      hosts: [example.com] # covered by the wildcard (it includes example.com)
      redirect_from: [www.example.com] # www.example.com/about → example.com/about
      redirects: # pages that moved, inside this site
        /feed: "https://feeds.example.net/blog" # temporary
        /2019: { to: /archive/2019, permanent: true } # permanent

  # One part of a site goes to another app: example.com/api
  api:
    clients: [office, home] # if office is down, home takes over
    target: 9000
    web:
      hosts: [example.com]
      paths: [/api] # only example.com/api/... comes here
      app_path: /v1 # the app sees /v1/... instead of /api/...

  # Many apps under one name; uses the *.apps.example.com wildcard
  apps:
    client: office
    target: 3000
    web:
      hosts: ["*.apps.example.com"] # a.apps.example.com, b.apps.example.com... all come here

  # An app on this server itself
  login:
    client: self # runs on this server, no client needed
    target: 9091
    web: auth.example.com

  # Its own certificate through DNS, at another provider (corp.example.org → your DNS server)
  erp:
    client: office
    target: erp.local:8443
    web:
      hosts: [erp.corp.example.org]
      challenge: dns # internal name: only the DNS check can work
      target_tls: { ca: certs/corp-ca.pem } # the app speaks HTTPS, checked against your company CA

  # Behind Cloudflare in "Full (strict)" mode: Cloudflare connects to burrow over HTTPS, so burrow
  # still needs a certificate; the TLS check can't pass through Cloudflare, the HTTP check can
  news:
    client: home
    target: 8097
    web:
      hosts: [news.example.net]
      challenge: http # Let's Encrypt fetches a file on port 80, through Cloudflare

  # Uses your own wildcard file (*.intranet.example.org)
  portal:
    client: office
    target: 8085
    web: portal.intranet.example.org

  # A site with its own certificate file, written on the site itself
  partner:
    client: office
    target: 8086
    web:
      hosts: [partner.example.net]
      tls: { cert: certs/partner.pem, key: certs/partner.key }

  # Proxmox: HTTPS with a self-made certificate
  pve:
    client: home
    target: 192.168.1.10:8006
    web:
      hosts: [pve.example.com]
      target_tls: { insecure_skip_verify: true } # says plainly: the app's certificate is not checked
      max_body_size: 8GB # big ISO uploads

  # A NAS that keeps its own HTTPS; burrow only reads the name and passes it on
  nas:
    client: home
    target: 192.168.1.20:443
    proxy_protocol: v2 # the NAS sees the real visitor address
    web:
      hosts: [nas.example.com]
      tls: passthrough

  # A home dashboard without HTTPS
  dashboard:
    client: home
    target: 8095
    web:
      hosts: [dash.lan.example.com]
      tls: off # plain http only; burrow warns it is not encrypted

  # Behind Cloudflare, which does the HTTPS: http:// visitors are still sent to https://
  wiki:
    client: home
    target: 3001
    web:
      hosts: [wiki.example.net]
      tls: off
      https_redirect: true # uses Cloudflare's header to see who typed http://

  # A secure site that also answers on http:// (old devices)
  updates:
    client: office
    target: 8096
    web:
      hosts: [updates.example.com]
      https_redirect: false

  # A kiosk reached by the server's IP address (a short-lived certificate; automatic check).
  # validate notes it: trusted_proxies is set, so a proxy may be in front. It reaches burrow directly here.
  kiosk:
    client: home
    target: 8091
    web: 203.0.113.10

  # A site under maintenance: visitors see a "back soon" page
  store:
    client: office
    target: 8082
    web:
      hosts: [store.example.com]
      maintenance: true

  # A normal TCP service, not a website
  db:
    client: home
    target: 5432
    visitors: [laptop] # only "laptop" can reach it, with burrow visit
```

Where each name gets its certificate:

- **The `*.example.com` wildcard** (with `example.com`): shop, blog, api, login, pve, updates, store, the tunnel, and the names that only redirect `www.example.com` and `old-blog.example.com`.
- **The `*.apps.example.com` wildcard:** apps.
- **Its own Let's Encrypt certificate:** erp (the DNS check at the company server), news (the HTTP check, through Cloudflare), kiosk (an IP address, short-lived; automatic check) and `example.io`, which only redirects.
- **Your files:** portal (the shared file) and partner (its own `tls` file).
- **None from burrow:**
  - nas (passthrough: the NAS keeps its own certificate);
  - dashboard and wiki (`tls: off`; Cloudflare does wiki's HTTPS);
  - db (not a website).

## What validate checks

| Problem | What validate says |
| --- | --- |
| a service has `web` but there is no top-level `web` | `services.blog.web: the web proxy is off; add a top-level web: with its ports, like web: {https: 443, http: 80}` |
| `web` has no port | `web needs a port: https for HTTPS sites, like https: 443, or http for plain HTTP sites, like http: 80` |
| a website with `tls: off` but no `web.http` | `services.wiki.web.tls: off serves plain HTTP, but web.http is not set; add http: 80` |
| an HTTPS choice (`tls: auto`, `passthrough` or `{cert, key}`, or a `challenge`) with no `web.https` | `services.blog.web.tls: auto needs web.https; without web.https every site is plain HTTP. Add https: 443, or remove tls` |
| `https_redirect: false` with no `web.http` | `services.blog.web.https_redirect: false serves plain HTTP too, but web.http is not set; add http: 80` |
| `https_redirect: false` on passthrough | `services.nas.web.https_redirect: false: burrow cannot serve plain HTTP for a passthrough service (the app keeps its own TLS); only the redirect is possible` |
| an HTTPS name with no certificate | `host "blog.example.com" (service "blog") has no certificate; add web.letsencrypt for automatic certificates, tls: {cert, key} on service "blog", or an entry in web.certificates whose hosts lists it` |
| `web.letsencrypt` with a wrong field | `web.letsencrypt.email "ops" is not an email address`; `web.letsencrypt.directory "stagging" is not production, staging or an https:// URL` (with `did you mean "staging"?`); `web.letsencrypt.ca: cannot read certs/ca.pem: file not found`, or `... certs/ca.pem holds no certificate` |
| Let's Encrypt cannot check a name | `blog.example.com (service "blog") cannot be checked by Let's Encrypt: open http: 80, add challenge: dns, or list a file for it in web.certificates` |
| a check without its port | `services.blog.web.challenge: http needs web.http on port 80: Let's Encrypt fetches a file there; add http: 80, or use challenge: tls or dns` |
| a check where no certificate is ordered | `services.nas.web.challenge: the app keeps its own certificate (tls: passthrough); remove challenge` |
| an entry in `web.certificates` without its files | `web.certificates[0] needs hosts, cert and key: hosts names which names use the file, cert and key are the files` |
| a wildcard from Let's Encrypt without a DNS provider | `services.apps.web.hosts: "*.apps.example.com" needs DNS-01 (Let's Encrypt gives wildcard certificates only through DNS): add web.letsencrypt.dns, or a certificate file under web.certificates or tls` |
| a DNS check under no provider's domain | `services.erp.web.hosts: "erp.corp.example.org" needs a DNS check (challenge: dns), but no web.letsencrypt.dns entry lists corp.example.org (or a domain above it)` |
| a DNS entry without one provider, or without domains | `web.letsencrypt.dns[1]: name exactly one of cloudflare, rfc2136, acme_dns or command`; `web.letsencrypt.dns[1].domains is required: the domains this provider manages, like domains: [example.com]` |
| a wildcard in `domains`, or a domain twice | `web.letsencrypt.dns[0].domains: write the domain example.com; it already covers *.example.com and every name under it`; `domain "example.com" is listed in web.letsencrypt.dns[0] and [2]; list it in one entry only` |
| a shared wildcard on the wrong entry | `web.letsencrypt.dns[0].wildcards: "*.apps.example.com" belongs to apps.example.com, which dns[1] manages; list it in dns[1].wildcards` |
| a DNS secret written in the config | `web.letsencrypt.dns[0].cloudflare.token is written in the file; use ${CF_TOKEN} or file: so it stays out of the config` |
| a DNS hook that cannot run | `web.letsencrypt.dns[3].command: /usr/local/bin/burrow-dns does not exist or is not executable` |
| note: three sites that one wildcard would cover | `wiki, blog and auth.example.com each get their own certificate; one wildcard would cover them: add "*.example.com" to wildcards in the letsencrypt.dns entry for example.com` |
| a certificate file that cannot be used | `web.certificates[0]: cannot read cert "certs/home.pem": file not found`, or `... certs/home.key does not fit certs/home.pem` |
| a file that does not cover a name | `services.portal.web.tls: certs/portal.pem does not cover "www.portal.example.com"; add the name to the certificate, move the name to another service, or list it in web.certificates` |
| warning: a shared file covers names its `hosts` does not list | `web.certificates[0]: certs/corp.pem also covers "*.example.com", which its hosts does not list; it is used only for the listed names` |
| a name in two certificate entries | `host "files.example.com" is listed in web.certificates[0] and [2]; list it in one entry only` |
| a name that is not a host name | `services.blog.web.hosts: "https://blog.example.com" is not a host name; write the name alone, like blog.example.com` |
| a short name on an HTTPS site | `"nas" is a single-label name; ... use a full name like nas.home.example.com, or set tls: off` |
| a wildcard that is too wide | `services.apps.web.hosts: "*.com" is too wide; a wildcard needs a parent name, like *.example.com` |
| one name and path in two services | `host "example.com" is used by services "blog" and "shop"; give one of them other paths, like paths: [/api]` |
| a path with `*`, `?`, `#` or `;` | `services.api.web.paths: path "/api/*" must start with / and hold no ?, # or *, like /api` |
| `app_path` without `paths` | `services.api.web.app_path: app_path needs paths: it replaces the matched path, like paths: [/api] with app_path: /` |
| a redirect to something that is neither an address nor a path | `redirect target "new.example.com" must be a full URL (https://...) or a path (/...)` |
| a name that only redirects but is served | `web.redirect_names: "example.com/api" is also served by service "api"` |
| HTTP keys on a passthrough service | `services.nas.web.paths: burrow cannot see the requests of a passthrough service (tls: passthrough); remove paths, or let burrow handle TLS` |
| a header rule HTTP cannot carry | `services.blog.web.request_headers: "Transfer-Encoding" belongs to HTTP's own framing, which burrow handles; it cannot be set` |
| `https_redirect: true` on a plain site with no trusted proxy | `services.blog.web.https_redirect: true on a plain-HTTP site needs web.trusted_proxies: ...; add trusted_proxies, or set https_redirect: false` |
| `target_tls` with a CA it cannot read | `services.erp.web.target_tls.ca: cannot read certs/corp-ca.pem: file not found` |
| note: a proxy in front and the automatic TLS check | `a proxy is in front (web.trusted_proxies is set); if it does the HTTPS, like Cloudflare's proxy, Let's Encrypt's TLS check cannot reach burrow: set challenge: http (with http: 80) or challenge: dns on these sites: kiosk` |
| warning: the whole internet as a proxy | `web.proxy_protocol_from holds 0.0.0.0/0: every visitor would have to send a PROXY header` |
| one name in two passthrough services | `host "nas.example.com" is used by services "nas" and "nas2"; each host name leads to one service` |
| a passthrough name that a website uses too | `host "nas.example.com" is used by a passthrough service "nas" and by a website` |
| the tunnel's name without a shared port | `websocket.host is only used when websocket.listen is the same port as web.https or web.http; ...` |
| the tunnel's name has no certificate | `websocket.host "tunnel.example.com" has no certificate; add web.letsencrypt, websocket.tls, or an entry in web.certificates whose hosts lists it` |
| a UDP service with `web` | `service "dns" is udp, but web only works for tcp services; remove web:, or set protocol: tcp` |
| warning: a plain-HTTP site | `services.wiki: served as plain HTTP; anyone on the path can read and change its traffic` |
| warning: no `web.http` | `web.http is not set: http:// addresses get no answer` |
| warning: a low port burrow may not open | `web.https [::]:443: port 443 needs root or CAP_NET_BIND_SERVICE; burrow service install grants it, or run burrow as root` |
| another listener on the same port | `web.https [::]:443 is also the control port (listen ...)`, or the service or metrics port it clashes with |

## Limits

- A connection has 10 seconds to send its first TLS message, and at most 16 KiB of it is read before the name is known.
- Connections that have not shown a name yet are limited like the tunnel's own handshakes (`limits.max_handshakes`), with their own counter: a flood of web connections cannot block tunnel clients, and the other way round.
- A tunnel connection on the shared port has `limits.handshake_timeout_secs` in total for TLS, the WebSocket upgrade and burrow's own handshake.
- Stopping or reloading the server lets a request in flight finish for up to 10 seconds.
