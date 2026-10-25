# Recipes

Each recipe goes through the steps of [How it works](how-it-works.md).

## Examples

Each example is the web part of a `server.yaml`: add it to the `listen:` and `clients:` of [A first website](../web.md#a-first-website) (`office` is a second client, written like `home`). Every one of them is checked by burrow's tests.

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

`client: self` reaches its own target without a `self:` block; `self.allow` only limits what visitors may `reach` on the server ([services.md](../services.md)).

### Single sign-on with authentik

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
  forward_auth: # every site with forward_auth: true asks authentik first
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
      forward_auth: true
  outpost: # authentik's own paths on each protected name, open so the login can finish
    client: self
    target: 9000
    web:
      hosts: ["*.apps.example.com"]
      paths: [/outpost.goauthentik.io]
```

In authentik, create a Proxy provider in "Forward auth (domain level)" mode for `apps.example.com`, and assign it to the embedded outpost. burrow asks authentik's Caddy endpoint, which reads the original address from the `X-Forwarded-*` headers burrow sets; visitors' own copies of the `X-authentik-*` headers never reach the app.

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
