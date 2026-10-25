# A full example and what validate checks

Each part of the example goes through the steps of [How it works](how-it-works.md); [step 5](how-it-works.md#5-saving-a-config) says what happens when you save it.

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
| an entry in `web.certificates` without its files | `web.certificates[0]: needs cert and key, or an uploaded certificate` |
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
