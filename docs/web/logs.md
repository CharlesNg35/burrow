# Access log and request ids

```yaml
# server.yaml
web:
  https: 443
  letsencrypt:
    email: ops@example.com
  access_log:
    file: logs/access.log
    format: json # or common (the default, like nginx and Apache)
  request_id: true # an X-Request-Id for each request
```

- One line per request: time, client address, name, method, path, protocol, status, bytes, time taken, service and request id. `access_log: logs/access.log` is the short form.
- `request_id` gives each request an `X-Request-Id`: sent to the app, returned to the visitor and written to the log. One sent by a trusted proxy is kept; a visitor's own is replaced.
