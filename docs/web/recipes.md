# Recipes

Short, complete configs for common cases and well-known apps. Copy the one closest to what you want, and change the names. Each recipe goes through the steps of [How it works](how-it-works.md).

## Examples

Each example is the web part of a `server.yaml`: add it to the `listen:` and `clients:` of [A first website](../web.md#a-first-website) (`office` is a second client, written like `home`). Every one of them is checked by burrow's tests.

### The minimum: websites with automatic HTTPS

One app, one name, a certificate from Let's Encrypt:

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

A site reached as both `example.com` and `www.example.com`:

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

A service's names share one certificate, as `example.com` and `www.example.com` do above. Adding or removing a name orders a new certificate for the new set.

### Your own certificates only (Let's Encrypt never contacted)

When you already have a certificate, here a wildcard, and want nothing ordered:

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

Several sites under `example.com` share one Let's Encrypt wildcard, got through Cloudflare's DNS:

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

Several apps, on two clients, under a wildcard file you already have:

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

No HTTPS port and no certificates, for a site on your home network:

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

`allow_from` checks the address the server sees. It keeps the site to your network only when the burrow server is on that network too; a server on the internet sees your router's public address instead.

### An app on the server itself

An app that runs on the burrow server, like a Git server, needs no client:

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

`client: self` reaches its own target without a `self:` block; `self.allow` only limits what visitors may `reach` on the server ([services.md](../services.md)).

### Single sign-on with authentik

Every app under `*.apps.example.com` behind one [authentik](https://goauthentik.io/) login:

```yaml
# server.yaml
web:
  https: 443
  letsencrypt:
    email: you@example.com
    dns:
      - domains: [example.com]
        cloudflare: { token: "${CF_TOKEN}" }
        wildcards: ["*.apps.example.com"]
  forward_auth: # login services, by name; a site picks one with forward_auth: authentik
    authentik:
      service: authentik
      path: /outpost.goauthentik.io/auth/caddy
      copy_headers: [X-authentik-username, X-authentik-groups, X-authentik-email, X-authentik-name, X-authentik-uid]
services:
  authentik: # the login portal itself, never behind a login
    client: self
    target: 9000
    web: auth.example.com
  apps: # every app under *.apps.example.com, behind the login
    client: office
    target: 3000
    web:
      hosts: ["*.apps.example.com"]
      forward_auth: authentik
  outpost: # authentik's own paths on each protected name, open so the login can finish
    client: self
    target: 9000
    web:
      hosts: ["*.apps.example.com"]
      paths: [/outpost.goauthentik.io]
```

In authentik, create a Proxy provider in "Forward auth (domain level)" mode for `apps.example.com`, and assign it to the embedded outpost. burrow asks authentik's Caddy endpoint, which reads the original address from the `X-Forwarded-*` headers burrow sets. Visitors' own copies of the `X-authentik-*` headers never reach the app.

### Home Assistant

[Home Assistant](https://www.home-assistant.io/) on its own name:

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

Home Assistant's pages and apps keep a WebSocket open (`/api/websocket`), which burrow passes through like any other request. Home Assistant blocks requests from a reverse proxy it does not trust, as its [HTTP integration docs](https://www.home-assistant.io/integrations/http/) say. So in Home Assistant, open Settings > System > Network, and under HTTP server turn on Trust X-Forwarded-For. Then add the address of the machine that runs the burrow client to Trusted proxies (`127.0.0.1` when the client runs on the Home Assistant machine).

### Nextcloud behind Cloudflare

Nextcloud, with Cloudflare's proxy in front and big uploads allowed:

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

In Nextcloud's `config.php`:

- trust the address the burrow client connects from (`'trusted_proxies' => ['127.0.0.1']` when it runs on the same machine);
- add `cloud.example.com` to `trusted_domains`;
- set `'overwriteprotocol' => 'https'`.

Nextcloud then reads the visitor's address from `X-Forwarded-For`, which burrow fills from Cloudflare's.

### Grafana under a path

Grafana at `example.com/grafana`, next to the main website:

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

With no domain at all, Let's Encrypt gives a short-lived certificate for the server's public IP address:

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

`listen: 7835` on the server is the tunnel's own port, for clients and visitors; it is not a website port. Clients can also come in on 443: see [The tunnel on the web ports](tunnel.md).

## Next steps

- [A full example](full-example.md): every website key in one commented config.
- [The tunnel on the web ports](tunnel.md): let clients connect on 443 too.
