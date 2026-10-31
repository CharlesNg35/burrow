# Access log and request ids

An access log writes one line per request: who asked for what, the answer, and how long it took. Use it to see who visits, find errors, or feed a tool like GoAccess or fail2ban. A request id ties the log line to the app's own log for the same request.

In [How it works](how-it-works.md): the request id and the log line belong to [step 2](how-it-works.md#2-a-request-is-answered).

This config logs every request of every site as JSON, with a request id:

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
- `request_id` gives each request an `X-Request-Id`, sent to the app, returned to the visitor and written to the log. An id from a trusted proxy is kept; a visitor's own is replaced.

An installed service may write the access log only in its own log folder; see [service.md](../service.md).

## Next steps

- [Monitoring](../monitoring.md): counts of answers per website, in `burrow status` and the metrics.
