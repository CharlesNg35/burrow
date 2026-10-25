# Health checks for websites

In [How it works](how-it-works.md): a service whose clients are all down answers `503` in [step 2](how-it-works.md#2-a-request-is-answered).

`health_check` (see [services.md](../services.md)) checks through each client that the app answers. For a website, `path` makes it ask the app for a page instead of only opening a connection:

```yaml
# server.yaml
web:
  https: 443
  letsencrypt:
    email: ops@example.com
services:
  blog:
    clients: [home, office] # the check decides which client gets new visitors
    target: 8080
    health_check: { path: /healthz }
    web: blog.example.com
```

- The check sends `GET /healthz` through each client, with `Host` set to the site's first name (the base name for a wildcard), `User-Agent: burrow-health-check/<version>` and nothing else. An answer of 2xx or 3xx counts as up; redirects are not followed.
- It reaches the app the way visitors do: through `target_tls` with the same certificate check, and over HTTP/2 with `target_http2`. It skips burrow's own site rules and is not written to the access log.
- **The path must not need a login:** 401 and 403 count as down. Pick a page the app serves to anyone, like a status endpoint.
- `burrow status` and the admin UI show why a client counts as down.
- `path` needs a website burrow serves: validate refuses it on a service without `web:`, on a passthrough site and with the web proxy off.
