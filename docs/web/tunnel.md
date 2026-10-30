# The tunnel on the web ports

Some networks let only web traffic out, on ports 443 and 80. burrow clients and visitors can then connect to the server on those same ports, sharing them with your websites: the tunnel gets a name of its own, or a path on a website's name, and a certificate like any site. This page shows the common setups; [websocket.md](../websocket.md#automatic-https-for-the-tunnel) explains every case.

In [How it works](how-it-works.md): the tunnel takes its name in [step 1](how-it-works.md#1-a-connection-arrives-on-the-https-port), its path in [step 2](how-it-works.md#2-a-request-is-answered), and port 80 in [step 6](how-it-works.md#6-port-80).

## The tunnel on the same port

burrow clients can connect over `wss://` on the same port as the websites, to a name of their own. The server shares port 443, and the client uses the tunnel's URL:

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
- The tunnel can also use a website's name, taking only its `path` there, or share the plain port `web.http` over `ws://` (still encrypted by burrow). See [websocket.md](../websocket.md#automatic-https-for-the-tunnel).

### The tunnel on port 8443, no port 80, through DNS

When ports 443 and 80 are taken or blocked, the tunnel can use another port and get its certificate through a DNS check:

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

When only port 80 gets through, the tunnel can use plain `ws://` there. burrow's own encryption still protects the data:

```yaml
# server.yaml
web:
  http: 80
websocket:
  listen: 80
  host: tunnel.example.com
  path: /burrow # clients use ws://tunnel.example.com/burrow
```

## Next steps

- [Servers reachable only through HTTP(S)](../websocket.md): WebSocket behind nginx, Caddy or Cloudflare.
