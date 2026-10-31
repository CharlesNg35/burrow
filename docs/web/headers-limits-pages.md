# Headers, limits and pages

These keys shape what a site sends and accepts: extra headers, the biggest upload, how long the app may take, your own error pages, a maintenance page, and HTTPS to an app that only speaks HTTPS. Most sites need none of them.

In [How it works](how-it-works.md): [step 2](how-it-works.md#2-a-request-is-answered) says when each key applies and which page answers.

Each key works under `web:` for every site, or in a service's `web:` for that site. The site's own value wins, except `request_headers` and `response_headers`, which are added to those under `web:`. This config sets defaults for every site, lets Proxmox take big uploads, and puts a shop in maintenance:

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

- **`request_headers` and `response_headers`** set headers, as `Name: value`, on requests to the app and on answers to visitors. An empty value removes a header. `Host` may be set for an app that wants its own name; `X-Forwarded-Host` keeps the visitor's. Headers of HTTP's own framing, like `Connection` or `Transfer-Encoding`, cannot be set. The admin UI, `burrow mcp`, change diffs and the server's log show the value of `Authorization`, `Proxy-Authorization`, `Cookie`, `Set-Cookie` and any header whose name ends in `-Key`, `-Token`, `-Secret`, `-Password` or `-Auth` as hidden, by name only; a `${VAR}` value shows its variable name.
- **`max_body_size`** has no limit by default: bodies are streamed to the app, never held in memory. A larger `Content-Length` gets 413 before the app is asked, without a `100 Continue`.
- **`error_pages`** replaces burrow's built-in pages with yours; a page missing from the folder stays built in. The folder is read when the config is saved.
- **`hsts`** is never sent on port 80.
- **`compress`** gzips HTML, CSS, JavaScript, JSON, XML, SVG and wasm answers of 1 KiB or more (or of unknown length). It does so only when the browser accepts gzip, and the app has not already compressed the answer or marked it `no-transform`.

### HTTPS to the app

Some apps, like Proxmox, UniFi or a NAS admin page, answer only over HTTPS. `target_tls` makes burrow speak HTTPS to them, and says how their certificate is checked:

| `target_tls` | The app's certificate is |
| --- | --- |
| `true` | checked against the public CAs, for the name or IP address of the target |
| `{ca: certs/corp-ca.pem}` | checked against your CA only |
| `{insecure_skip_verify: true}` | not checked, for apps with a self-made certificate like Proxmox, UniFi or DSM |

An IP target needs its IP address in the app's certificate. A plain-HTTP app behind `target_tls` answers with a clear 502.

## Limits

These fixed limits apply to every website connection, so slow or broken visitors cannot tie up the server:

- A connection has `limits.handshake_timeout_secs` (5 seconds) to send its first TLS message and finish TLS, and at most 16 KiB of the first message is read before the name is known.
- Connections that have not finished TLS yet are limited like the tunnel's own handshakes (`limits.max_handshakes`), with their own counter. Over the limit a connection waits its turn within its deadline instead of being closed, and one address that stalls its connections loses them once the server is busy ([how it works](how-it-works.md)). A flood of web connections cannot block tunnel clients, and the other way round.
- One visitor cannot take all of a service's or a client's connections. Once more than half of a service's `max_connections`, or of its client's connections (`limits.max_connections_per_client`), are open, one visitor address (an IPv6 /64) may hold at most an eighth of them, and at least 8. Its further requests get `503` at once; everyone else keeps getting through. Behind `web.trusted_proxies` the visitor is the real address from `X-Forwarded-For`; loopback addresses have no share. There is nothing to set.
- A tunnel connection on the shared port has `limits.handshake_timeout_secs` in total for TLS, the WebSocket upgrade and burrow's own handshake.
- Stopping or reloading the server lets a request in flight finish for up to 10 seconds.

## Next steps

- [Who may visit](access.md): rate limits, passwords and logins.
- [All web keys with their defaults](../configuration.md#serveryaml).
