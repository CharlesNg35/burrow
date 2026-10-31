# Servers reachable only through HTTP(S)

Sometimes the server can be reached only on the web ports: it sits behind nginx, Caddy, Cloudflare or a Kubernetes ingress, or a firewall lets only HTTPS through. Clients and visitors can then connect over **WebSocket**, a standard way to keep a long-lived connection open through web servers and proxies.

WebSocket is off unless you turn it on. Everything works as over TCP, and the proxy in between sees only encrypted data. There are two ways to set it up:

- **Behind your own web server or proxy** (nginx, Caddy, Cloudflare, Kubernetes): burrow listens on a local port, and the web server forwards `wss://` to it. See [Setup](#setup) and the [configs for common proxies](#examples).
- **burrow on port 443 itself**, with a certificate it gets from Let's Encrypt: see [Automatic HTTPS for the tunnel](#automatic-https-for-the-tunnel).

This is how the first way looks:

```
client ══ wss://tunnel.example.com/burrow ══> nginx or Caddy (TLS) ──> burrow server on 127.0.0.1:7836
```

## Setup

These steps put burrow behind a web server you already run. On the server, write a config with WebSocket on a local port:

```sh
./burrow init server --ws --server tunnel.example.com   # WebSocket on 127.0.0.1:7836/burrow, no TCP port for clients
```

```yaml
# server.yaml
public_address: tunnel.example.com
websocket:
  listen: 127.0.0.1:7836   # a port alone would mean 0.0.0.0
  path: /burrow
```

Then point your web server at `127.0.0.1:7836` ([examples below](#examples)). Clients and visitors use the URL as their server:

```yaml
# client.yaml
server: wss://tunnel.example.com/burrow   # wss:// is WebSocket over TLS (HTTPS); ws:// has no TLS
```

`burrow client-config` writes this URL from `public_address`.

- `listen` (TCP) and `websocket` can be on together, for example while you move clients from one to the other.
- Idle connections send a keepalive every 25 seconds (`websocket.keepalive_secs`, 5 to 30), shorter than the idle timeouts of nginx and Cloudflare.
- Services exposed with `bind` are still plain TCP or UDP ports on the server. Only the connection between client and server goes over WebSocket.
- `headers` in client.yaml or visitor.yaml adds HTTP headers to the WebSocket connection, for example an access token for a proxy.
- A client that must go through an outgoing proxy uses `proxy: http://host:port` or `socks5://host:port`.
- burrow can also do the tunnel's HTTPS itself, with a Let's Encrypt certificate, and share port 443 (or 80) with websites: see [Automatic HTTPS for the tunnel](#automatic-https-for-the-tunnel).

## Automatic HTTPS for the tunnel

When nothing else runs on the server's port 443, burrow can take it, and get and renew the tunnel's certificate from Let's Encrypt, with no nginx or certbot. The tunnel then shares port 443 with burrow's [web proxy](web.md), which may serve no websites at all. One command writes the config:

```sh
./burrow init server --ws --email you@example.com --server tunnel.example.com
```

The tunnel's part of the config looks like this:

```yaml
# server.yaml
web:
  https: 443
  letsencrypt:
    email: you@example.com # writing this accepts Let's Encrypt's terms
websocket:
  listen: 443 # the same as web.https: shared
  host: tunnel.example.com # gets and renews its certificate automatically
  path: /burrow
```

Point `tunnel.example.com` at the server, open port 443, and start. Clients use `wss://tunnel.example.com/burrow`, which `burrow client-config` prints. Other names on port 443 are closed until you add websites ([web.md](web.md)).

- **Your own files still work:** `websocket.tls` with `cert` and `key` takes precedence over Let's Encrypt, and a file renewed on disk is used from the next reload.
- **Another HTTPS port:** sharing needs `websocket.listen` to equal `web.https` in port and address, like `8443` and `8443`. Clients then use `wss://tunnel.example.com:8443/burrow`. Let's Encrypt's TLS and HTTP checks reach only ports 443 and 80. So on another port, add `http: 80` (with port 80 open), use the DNS check, or use your own files; `burrow validate` says so.
- **Behind a home router:** burrow sees only the port it listens on. If the router forwards the internet's 443 to burrow's 8443, Let's Encrypt's check would reach burrow, but burrow cannot know that. So validate still asks for port 80, the DNS check or files: forward port 80 to `web.http: 80` as well, or use the DNS check ([Port forwarding at home](web/proxies.md#port-forwarding-at-home)).
- **The DNS check, with no open port:** `websocket.challenge: dns` gets the tunnel's certificate through a DNS provider in `web.letsencrypt.dns`, so neither 80 nor 443 has to be reachable from the internet ([DNS checks](web/certificates.md#dns-checks-wildcards-and-names-the-internet-cannot-reach)):

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
    challenge: dns # clients use wss://tunnel.example.com:8443/burrow
  ```
- **Port 80 instead:** `websocket.listen` may equal `web.http`, like `80`. Clients then use `ws://tunnel.example.com/burrow`. No certificate is needed: burrow's own encryption still protects the tunnel's data, and only the host, path, sizes and timing are visible on the network.

  ```yaml
  # server.yaml
  web:
    http: 80
  websocket:
    listen: 80 # the same as web.http: shared over plain ws://
    host: tunnel.example.com
    path: /burrow
  ```

- **A website's name:** `websocket.host` may also be the name of a website burrow serves over HTTPS. The tunnel then takes only `path` (and below) on that name, and uses the website's certificate.
- **Its own port:** without `host`, `websocket.listen` is a port of its own, as in the setup above. Plain `ws://` on port 443 works but gets a note from validate: networks that expect TLS there may block it.

## Examples

Ready configs for the proxies burrow most often sits behind. Each forwards `wss://tunnel.example.com/burrow` to burrow on `127.0.0.1:7836`, as in [Setup](#setup). The last one has burrow serve `wss://` itself, with your own certificate files.

<details>
<summary><b>nginx</b></summary>

```nginx
# /etc/nginx/conf.d/burrow.conf
map $http_upgrade $connection_upgrade {
    default upgrade;
    ''      close;
}

server {
    listen 443 ssl;
    server_name tunnel.example.com;
    ssl_certificate     /etc/letsencrypt/live/tunnel.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/tunnel.example.com/privkey.pem;

    location = /burrow {
        proxy_pass http://127.0.0.1:7836;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $connection_upgrade;
        proxy_set_header Host $host;
        proxy_read_timeout 1h;
        proxy_send_timeout 1h;
        proxy_buffering off;
    }
}
```

Client: `server: wss://tunnel.example.com/burrow`

</details>

<details>
<summary><b>Caddy</b></summary>

Caddy gets the certificate itself and proxies WebSockets without extra settings:

```caddy
# Caddyfile
tunnel.example.com {
    reverse_proxy /burrow 127.0.0.1:7836
}
```

Client: `server: wss://tunnel.example.com/burrow`

</details>

<details>
<summary><b>Cloudflare (proxied DNS)</b></summary>

Point the DNS record at your server with the proxy enabled (orange cloud), and run nginx or Caddy on the origin as above. WebSockets are allowed on every plan. Use SSL mode "Full (strict)" so the hop between Cloudflare and your origin is also TLS.

To restrict the endpoint with [Cloudflare Access](https://developers.cloudflare.com/cloudflare-one/identity/service-tokens/), create a service token and send it from the client:

```yaml
# client.yaml (or visitor.yaml)
server: wss://tunnel.example.com/burrow
headers:
  CF-Access-Client-Id: ${CF_ACCESS_ID}
  CF-Access-Client-Secret: ${CF_ACCESS_SECRET}
```

</details>

<details>
<summary><b>Cloudflare Tunnel (no open inbound port at all)</b></summary>

`cloudflared` makes an outbound connection to Cloudflare, so the server needs no open ports for the control link:

```yaml
# /etc/cloudflared/config.yml
tunnel: <tunnel-id>
credentials-file: /etc/cloudflared/<tunnel-id>.json
ingress:
  - hostname: tunnel.example.com
    path: ^/burrow$
    service: http://127.0.0.1:7836
  - service: http_status:404
```

Client: `server: wss://tunnel.example.com/burrow`. Services exposed with `bind` are still plain TCP/UDP ports on the server, so open those (or bind them to a private interface) as needed.

</details>

<details>
<summary><b>Kubernetes (ingress-nginx)</b></summary>

```yaml
# ingress.yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: burrow
  annotations:
    nginx.ingress.kubernetes.io/proxy-read-timeout: "3600"
    nginx.ingress.kubernetes.io/proxy-send-timeout: "3600"
spec:
  ingressClassName: nginx
  tls:
    - hosts: [tunnel.example.com]
      secretName: tunnel-example-com-tls
  rules:
    - host: tunnel.example.com
      http:
        paths:
          - path: /burrow
            pathType: Exact
            backend:
              service:
                name: burrow
                port:
                  number: 7836
```

Bind `websocket.listen` to `0.0.0.0:7836` inside the pod so the Service can reach it.

</details>

<details>
<summary><b>No proxy: burrow serves wss:// itself</b></summary>

Give burrow a certificate and key (for example from Let's Encrypt):

```sh
./burrow init server \
  --ws-tls-cert /etc/letsencrypt/live/tunnel.example.com/fullchain.pem \
  --ws-tls-key  /etc/letsencrypt/live/tunnel.example.com/privkey.pem \
  --server tunnel.example.com
```

```yaml
# server.yaml
public_address: tunnel.example.com
websocket:
  listen: 0.0.0.0:7837
  path: /burrow
  tls:
    cert: /etc/letsencrypt/live/tunnel.example.com/fullchain.pem
    key: /etc/letsencrypt/live/tunnel.example.com/privkey.pem
```

Clients use `wss://tunnel.example.com:7837/burrow`. Relative file paths are resolved from the config file's directory. The key must be readable by the user burrow runs as. A renewed certificate is used without a restart. A renewal writes a new key file, so give the service account access again each time, for example with `--deploy-hook "setfacl -m u:burrow:r /etc/letsencrypt/live/tunnel.example.com/privkey.pem"` in certbot.

For a self-signed certificate, copy it to the client and trust it there with `ca: server-cert.pem`. `ca` takes either a CA that signed the server's certificate, or the server's certificate itself. That exact certificate is then accepted although no public CA signed it. Certificates signed by public CAs are still accepted too: `ca` adds trust but does not limit it.

</details>

## Next steps

- [The tunnel on the web ports](web/tunnel.md): more ways to share 443 or 80 with websites.
- [Websites](web.md): serve your apps on the same port.
- [Security](security.md): what the proxy in between can and cannot see.
