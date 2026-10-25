# Headers, limits and pages

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

## Limits

- A connection has 10 seconds to send its first TLS message, and at most 16 KiB of it is read before the name is known.
- Connections that have not shown a name yet are limited like the tunnel's own handshakes (`limits.max_handshakes`), with their own counter: a flood of web connections cannot block tunnel clients, and the other way round.
- A tunnel connection on the shared port has `limits.handshake_timeout_secs` in total for TLS, the WebSocket upgrade and burrow's own handshake.
- Stopping or reloading the server lets a request in flight finish for up to 10 seconds.
