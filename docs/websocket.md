# Servers reachable only through HTTP(S)

Sometimes the server can only be reached on the usual web ports, for example because it sits behind nginx, Caddy, Cloudflare or a Kubernetes ingress, or because a firewall only lets HTTPS through. Clients and visitors can then connect over **WebSocket**, a standard way to keep a long-lived connection open through web servers and proxies.

WebSocket is off unless you turn it on. Everything works exactly as over TCP, and the proxy in between only sees encrypted data.

```
client ══ wss://tunnel.example.com/burrow ══> nginx or Caddy (TLS) ──> burrow server on 127.0.0.1:7836
```

## Setup

On the server:

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

Then point your web server at `127.0.0.1:7836` (examples below). Clients and visitors use the URL as their server:

```yaml
# client.yaml
server: wss://tunnel.example.com/burrow   # wss:// is WebSocket over TLS (HTTPS); ws:// has no TLS
```

`burrow client-config` writes this URL from `public_address`.

- `listen` (TCP) and `websocket` can be on together, for example while you move clients from one to the other.
- Idle connections send a keepalive every 25 seconds (`websocket.keepalive_secs`, 5 to 30), shorter than the idle timeouts of nginx and Cloudflare.
- The services you expose with `bind` are still plain TCP or UDP ports on the server. Only the connection between client and server goes over WebSocket.
- `headers` in client.yaml or visitor.yaml adds HTTP headers to the WebSocket connection, for example an access token for a proxy.
- A client that must go through an outgoing proxy uses `proxy: http://host:port` or `socks5://host:port`.
- burrow can also do HTTPS for the tunnel itself, with a certificate from Let's Encrypt, and share port 443 (or 80) with websites: see [Automatic HTTPS for the tunnel](#automatic-https-for-the-tunnel).

## Automatic HTTPS for the tunnel

burrow can get and renew the tunnel's certificate from Let's Encrypt by itself, with no nginx or certbot. The tunnel then shares port 443 with the web proxy, which may serve no websites at all:

```sh
./burrow init server --ws --email you@example.com --server tunnel.example.com
```

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
- **Another HTTPS port:** sharing needs `websocket.listen` to equal `web.https` in port and address, like `8443` and `8443`; clients then use `wss://tunnel.example.com:8443/burrow`. Let's Encrypt only checks ports 443 and 80, so on another port add `http: 80` (with port 80 open) or use your own files; `burrow validate` says so.
- **Behind a home router:** burrow only sees the port it listens on. If the router forwards the internet's 443 to burrow's 8443, Let's Encrypt's check would reach burrow, but burrow cannot know that, so validate still asks for port 80 or files. Forward port 80 to `web.http: 80` as well.
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

- **A website's name:** `websocket.host` may also be the name of a website burrow serves over HTTPS; the tunnel then takes only `path` (and below) on that name and uses the website's certificate.
- **Its own port:** without `host`, `websocket.listen` is a port of its own, as in the setup above. Plain `ws://` on port 443 works but gets a note from validate: networks that expect TLS there may block it.

## Examples

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

Client: `server: wss://tunnel.example.com/burrow`. The services you expose with `bind` are still plain TCP/UDP ports on the server, so open those (or bind them to a private interface) as needed.

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

Clients use `wss://tunnel.example.com:7837/burrow`. Relative file paths are resolved from the config file's directory. The key must be readable by the user burrow runs as. A renewed certificate is used without a restart; since a renewal writes a new key file, give the service account access again each time, for example with `--deploy-hook "setfacl -m u:burrow:r /etc/letsencrypt/live/tunnel.example.com/privkey.pem"` in certbot.

For a self-signed certificate, copy it to the client and trust it there with `ca: server-cert.pem`. `ca` takes either a CA that signed the server's certificate, or the server's certificate itself. That exact certificate is then accepted even though no public CA signed it, and certificates signed by public CAs are still accepted too, so `ca` adds trust but does not limit it.

</details>
