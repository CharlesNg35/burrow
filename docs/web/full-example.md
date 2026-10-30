# A full example and what validate checks

This page shows every website setting burrow has, in one `server.yaml` you could run. It is the config of one person's burrow server, a machine with a public address in front of a home and a small office, with a public blog and shop, family apps behind logins, a few devices that keep their own HTTPS, sites only for the home network, and some old names that redirect. Each key has a comment on why you would use it. You never need all of them: most sites need only `client`, `target` and `web: name`, as in [A first website](../web.md#a-first-website). Use this page to find a key and see it in context; the topic pages explain each part.

Each part of the example goes through the steps of [How it works](how-it-works.md); [step 5](how-it-works.md#5-saving-a-config) says what happens when you save it.

## A full example

The config is in four parts: the tunnel, the web settings for every site, certificates, and the sites, grouped by what they do.

```yaml
# server.yaml
# ─────────────────────────────────────────────────────────────
# 1. THE TUNNEL: how clients and visitors reach this server
# ─────────────────────────────────────────────────────────────
listen: 7835 # clients and visitors connect here over TCP
public_address: tunnel.example.com # written into the configs client-config prints

websocket: # clients on networks that only let web traffic out connect over wss://
  listen: 443 # the same port as the websites: shared, told apart by name
  host: tunnel.example.com # this name is the tunnel; it uses the *.example.com wildcard below
  path: /burrow

clients: # the machines that run your apps
  home:
    secret: ${HOME_SECRET}
  office:
    secret: ${OFFICE_SECRET}

visitors: # your own devices, which reach private services with "burrow visit"
  laptop:
    secret: ${LAPTOP_SECRET}

# ─────────────────────────────────────────────────────────────
# 2. THE WEB PROXY: ports and defaults for every site
#    Without this "web:" block, burrow serves no websites.
# ─────────────────────────────────────────────────────────────
web:
  # Ports
  https: 443 # HTTPS sites; leave it out and every site is plain HTTP
  http: 80 # http://: redirects to https, plain sites, and Let's Encrypt's HTTP check
  https_redirect: true # HTTPS sites send http:// visitors to https:// (the default)
  http2: true # browsers may use HTTP/2, faster for pages with many parts (the default)

  # What is in front of burrow
  trusted_proxies: [173.245.48.0/20] # Cloudflare proxies some names: believe its "real visitor" headers
  proxy_protocol_from: [10.0.0.5/32] # a TCP load balancer that sends a PROXY header with the visitor's address

  # Time limits
  request_header_timeout_secs: 10 # a visitor must send its request within 10 s; slow senders are dropped
  idle_timeout_secs: 60 # a connection that moves nothing for a minute is closed; WebSockets stay open
  response_header_timeout_secs: 60 # the app must start answering within 60 s, else 504

  # Defaults for every site; each site can set its own
  max_body_size: 100MB # the biggest upload (no limit if you leave it out)
  compress: true # gzip text pages: smaller and faster to load
  hsts: true # browsers always use https for these names (hard to undo!)
  rate_limit: 50 # requests per second per visitor address: slows scrapers and floods
  request_headers: # sent to every app
    X-Served-By: burrow
  response_headers: # sent to every browser
    X-Content-Type-Options: nosniff # stops browsers guessing file types
    Server: "" # empty: removes the header, so the app's software is not named
  error_pages: pages/ # your own 404.html, 413.html, 429.html, 502.html, 503.html, 504.html, maintenance.html

  # Logs
  access_log: # one line per request: who, what, the answer and how long it took
    file: logs/access.log
    format: json # or common, like nginx and Apache
  request_id: true # an id per request, in the log and sent to the app, to match both logs

  # Single sign-on: login services by name; a site picks one with forward_auth: <name>
  forward_auth:
    authelia:
      service: login # a service below
      path: /api/authz/forward-auth # where Authelia answers "may this visitor in?"
      copy_headers: [Remote-User, Remote-Groups] # passed to the app on a yes; visitors' own copies are removed

  # Old names with no site any more: they only send visitors away
  redirect_names:
    old-blog.example.com: "https://example.com" # temporary (307): path and query kept
    example.io: { to: "https://example.com", permanent: true } # permanent (308): browsers remember it

# ─────────────────────────────────────────────────────────────
# 3. CERTIFICATES: where HTTPS sites get theirs
#    Order per name: the site's own tls file or upload, tls: internal,
#    web.certificates, a shared wildcard, then Let's Encrypt.
# ─────────────────────────────────────────────────────────────
  letsencrypt: # free automatic certificates; writing this accepts Let's Encrypt's terms
    email: you@example.com # your account, shown in burrow status
    # directory: staging # Let's Encrypt's test service: try a new setup without hitting its limits
    # profile: shortlived # 6-day certificates, renewed often, if you prefer them
    # For another ACME server, like your own step-ca server, directory takes its URL, and the CA
    # that signed its certificate goes in as a file path or, from the admin UI, as ca_upload: <id>.
    dns: # DNS providers: for wildcards, and for names the internet cannot reach
      - domains: [example.com, example.net] # Cloudflare runs DNS for these and every name under them
        cloudflare: { token: "${CF_TOKEN}" } # quoted: ${...} inside { } must be; a token for DNS of these zones only
        wildcards: ["*.example.com", "*.apps.example.com"] # shared certificates, got when you save: one order for many sites
        wait_secs: 120 # how long burrow waits for every nameserver to see the record (the default)
        resolvers: [1.1.1.1, 9.9.9.9] # used only to find the zone's nameservers; must give the public view
      - domains: [corp.example.org] # the office's own DNS server
        # 10.0.0.53 is the zone's primary; it feeds the public nameservers of corp.example.org,
        # because Let's Encrypt reads the record from the internet (only the name may be internal)
        rfc2136: { server: "10.0.0.53:53", key_name: acme, secret: "${TSIG_SECRET}", algorithm: hmac-sha256 }
      - domains: [example.io] # a registrar with no DNS API: acme-dns answers for it
        acme_dns:
          api: https://auth.acme-dns.example.org
          username: ${ACMEDNS_USER}
          password: ${ACMEDNS_KEY}
          subdomain: d420c923-bbd7-4056-ab64-c3ca54c9b3cf
      # Any other provider: a script of yours, called with present|cleanup NAME VALUE, like
      # command: /usr/local/bin/burrow-dns

  certificates: # your own certificate files, for names you already have them for
    - hosts: ["*.intranet.example.org"] # the names this file is used for, and only these
      cert: certs/intranet-wildcard.pem
      key: certs/intranet-wildcard.key
    # An entry uploaded in the admin UI holds an upload id instead of the two paths.

# ─────────────────────────────────────────────────────────────
# 4. SITES: a service with "web:" is a website
#    tls: auto (default) | internal | passthrough | off | { cert, key } | { upload }
#    challenge: dns | http | tls (optional; automatic when not set)
# ─────────────────────────────────────────────────────────────
services:

  # ── Public sites at home ──────────────────────────────────

  # The main site; "www." sends visitors to the bare name
  blog:
    client: home
    target: 8080
    web:
      hosts: [example.com] # covered by the *.example.com wildcard (it includes example.com)
      redirect_from: [www.example.com] # www.example.com/about → example.com/about, permanent
      redirects: # pages that moved, inside this site
        /feed: "https://feeds.example.net/blog" # temporary
        /2019: { to: /archive/2019, permanent: true } # permanent

  # One part of the site goes to another app: example.com/api
  api:
    clients: [office, home] # if office is down, home takes over
    target: 9000
    health_check: { path: /healthz } # asks the app for a page, so a broken app is skipped, not only a dead one
    web:
      hosts: [example.com]
      paths: [/api] # only example.com/api/... comes here; the longest path wins
      app_path: /v1 # the app sees /v1/... instead of /api/...

  # A shop with a stricter limit and its own headers
  shop:
    client: home
    target: 8081
    web:
      hosts: [shop.example.com]
      rate_limit: 10 # this site's own limit, tighter than web.rate_limit
      max_body_size: 5MB # product photos at most
      response_headers:
        Content-Security-Policy: "frame-ancestors 'none'" # the shop may not be framed by other sites

  # A site under maintenance: visitors see a "back soon" page (503 with Retry-After)
  store:
    client: office
    target: 8082
    web:
      hosts: [store.example.com]
      maintenance: true
      error_pages: store-pages/ # this site's own maintenance.html

  # ── Family apps behind a login ────────────────────────────

  # The login portal (Authelia) on the server itself; never behind a login
  login:
    client: self # runs on this server, no client needed
    target: 9091
    web: auth.example.com

  # A wiki behind the single sign-on above
  wiki:
    client: home
    target: 3001
    web:
      hosts: [wiki.example.com]
      forward_auth: authelia # asks web.forward_auth.authelia before each request

  # Shared files with a simple password instead
  files:
    client: home
    target: 8083
    web:
      hosts: [files.example.com]
      basic_auth: # user: hash, from "burrow passwd alice"; a site takes basic_auth or forward_auth, not both
        alice: "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHRzb21lc2FsdA$Q0gCQmVHjy2MKCgr4tvPPMkJEY3gvb2vmT6oGmP4ZgM"
      max_body_size: 2GB # big videos

  # Many small apps under one wildcard name, all behind the login
  apps:
    client: office
    target: 3000
    web:
      hosts: ["*.apps.example.com"] # a.apps.example.com, b.apps.example.com... all come here
      forward_auth: authelia

  # ── Apps that speak HTTPS or HTTP/2 themselves ────────────

  # Proxmox: HTTPS with a self-made certificate
  pve:
    client: home
    target: 192.168.1.10:8006
    web:
      hosts: [pve.example.com]
      target_tls: { insecure_skip_verify: true } # says plainly: the app's certificate is not checked
      max_body_size: 8GB # big ISO uploads
      compress: false # ISO and backup downloads are already compressed

  # An office app on an internal name, with an HTTPS certificate from the company CA
  erp:
    client: office
    target: erp.local:8443
    web:
      hosts: [erp.corp.example.org]
      challenge: dns # internal name: only the DNS check can work
      target_tls: { ca: certs/corp-ca.pem } # the app's certificate is checked against the company CA
      response_header_timeout_secs: 300 # its reports take minutes to start

  # A gRPC API: HTTP/2 all the way to the app
  grpc:
    client: office
    target: 50051
    health_check: true # gRPC apps get a connection check, not a page
    web:
      hosts: [grpc.example.com]
      target_http2: true # speak HTTP/2 to the app (plain h2c here), as gRPC needs
      request_headers:
        X-Api-Gateway: burrow # this app's own header, on top of web.request_headers

  # ── Sites behind Cloudflare ───────────────────────────────

  # Cloudflare in "Full (strict)" mode connects to burrow over HTTPS, so burrow still needs a
  # certificate; the TLS check can't pass through Cloudflare, the HTTP check can
  news:
    client: home
    target: 8097
    web:
      hosts: [news.example.net]
      challenge: http # Let's Encrypt fetches a file on port 80, through Cloudflare

  # Cloudflare does the HTTPS; http:// visitors are still sent to https://
  docs:
    client: home
    target: 3002
    web:
      hosts: [docs.example.net]
      tls: off
      https_redirect: true # uses Cloudflare's X-Forwarded-Proto to see who typed http://

  # ── Sites for the home network and old devices ────────────

  # The printer's page, for devices whose own DNS points printer.home.arpa at this server;
  # a certificate from burrow's local CA, installed once on each device
  printer:
    client: home
    target: 192.168.1.30:80
    web:
      hosts: [printer.home.arpa]
      tls: internal # LAN names only; Let's Encrypt never sees them

  # A dashboard for an old TV browser that cannot do today's HTTPS
  dashboard:
    client: home
    target: 8095
    web:
      hosts: [dash.lan.example.com]
      tls: off # plain http only; burrow warns it is not encrypted

  # A secure site that also answers on http://, for old devices
  updates:
    client: office
    target: 8096
    web:
      hosts: [updates.example.com]
      https_redirect: false

  # ── Your own certificates ─────────────────────────────────

  # Uses the *.intranet.example.org file listed under web.certificates
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

  # A certificate uploaded in the admin UI (My own certificate); burrow wrote the id
  status:
    client: office
    target: 8087
    web:
      hosts: [status.example.net]
      tls: { upload: pair-1a2b3c4d5e6f7a8b }

  # A kiosk reached by the server's IP address (a short-lived certificate; automatic check).
  # validate notes it: trusted_proxies is set, so a proxy may be in front. It reaches burrow directly here.
  kiosk:
    client: home
    target: 8091
    web: 203.0.113.10

  # ── Devices that keep their own HTTPS ─────────────────────

  # A NAS: burrow only reads the name and passes the encrypted bytes on
  nas:
    client: home
    target: 192.168.1.20:443
    proxy_protocol: v2 # the NAS sees the real visitor address
    web:
      hosts: [nas.example.com]
      tls: passthrough

  # Home Assistant, open only to devices with a certificate from your device CA
  ha:
    client: home
    target: 8123
    web:
      hosts: [ha.example.com]
      client_auth:
        ca: certs/devices-ca.pem # the CA that signs your phones' and laptops' certificates
        crl: certs/devices.crl # devices you revoked, like a lost phone
        # The admin UI writes ca_upload: <id> and crl_upload: <id> instead of these paths.

  # ── Not a website ─────────────────────────────────────────

  # A database only the laptop can reach, with burrow visit
  db:
    client: home
    target: 5432
    visitors: [laptop]

  # One app on two machines: new connections take turns, and office gets 3 of every 4
  game:
    clients: [{name: office, weight: 3}, home]
    mode: balance # or least_connections: each one goes to the machine with the fewest open
    health_check: true # a machine whose app does not answer gets none
    bind: 7850
    target: 7000

  # Two VMs behind the office machine: each entry has its own target
  reports:
    clients: [{name: office, target: 10.0.0.12:3000}, {name: office, target: 10.0.0.13:3000}]
    mode: least_connections # each new connection goes to the VM with the fewest open
    health_check: true
    bind: 7851
    target: 3000
```

Where each name gets its certificate:

- **The `*.example.com` wildcard** (with `example.com`): blog, api, shop, store, login, wiki, files, pve, grpc, updates, ha, the tunnel, and the names that only redirect `www.example.com` and `old-blog.example.com`.
- **The `*.apps.example.com` wildcard:** apps.
- **Its own Let's Encrypt certificate:** erp (the DNS check at the company server), news (the HTTP check, through Cloudflare), kiosk (an IP address, short-lived; automatic check), and `example.io`, which only redirects.
- **burrow's local CA:** printer.
- **Your files:** portal (the shared file), partner (its own `tls` file) and status (its uploaded file).
- **None from burrow:**
  - nas (passthrough: the NAS keeps its own certificate);
  - dashboard and docs (`tls: off`; Cloudflare does docs' HTTPS);
  - db (not a website).

## What validate checks

`burrow validate` checks every website setting before anything runs, and the same checks run at each start and save. These are the problems it finds, with the message it shows:

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
| a site names a login service that is not defined | `services.wiki.web.forward_auth: there is no login service "authelai" under web.forward_auth; the names are authelia, authelia-basic` (with `did you mean "authelia"?`) |
| `forward_auth: true` with two login services | `services.wiki.web.forward_auth: true is unclear with 2 login services under web.forward_auth; name one: authelia or authelia-basic` |
| `forward_auth` on the login service's own site | `services.login.web.forward_auth: login answers the logins of web.forward_auth.authelia, so its own site must stay open` |
| warning: a login service no site uses | `web.forward_auth.authentik: no website uses it; set forward_auth: authentik on a site, or remove it` |
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

## Next steps

- [Recipes](recipes.md): short configs for one task each.
- [All web keys with their defaults](../configuration.md#serveryaml).
