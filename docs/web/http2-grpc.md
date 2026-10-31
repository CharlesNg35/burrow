# HTTP/2 and gRPC

HTTP/2 is the newer version of HTTP that browsers use to load a page's many parts over one connection. burrow speaks it to browsers by default. gRPC, a way for programs to call each other over HTTP/2, works once the site also speaks HTTP/2 to the app. This page shows both, and the limits a long gRPC stream meets.

In [How it works](how-it-works.md): HTTP/2 is agreed in [step 1](how-it-works.md#1-a-connection-arrives-on-the-https-port); [step 2](how-it-works.md#2-a-request-is-answered) says what happens when a stream fails.

## HTTP/2

Browsers use HTTP/2 on `web.https` when they can; other clients keep HTTP/1.1. burrow talks to the app over HTTP/1.1 either way, unless the site sets `target_http2`. `web.http2: false` keeps every site on HTTP/1.1, for a client that breaks with HTTP/2. `burrow validate` shows which is on: `web: https [::]:443 (HTTP/2 and HTTP/1.1, IPv4 and IPv6)`.

- HTTP/2 needs HTTPS: plain-HTTP sites and port 80 speak HTTP/1.1 only.
- WebSockets keep working: browsers open them over HTTP/1.1 beside the HTTP/2 connection.
- burrow sets every HTTP/2 limit itself, against the attacks of recent years (see [security.md](../security.md#websites)).

## gRPC

This config serves a gRPC app from the office as `grpc.example.com`:

```yaml
# server.yaml
web:
  https: 443
  letsencrypt:
    email: ops@example.com
services:
  grpc:
    client: office
    target: 50051
    web:
      hosts: [grpc.example.com]
      target_http2: true # speak HTTP/2 to the app: plain h2c here, h2 inside target_tls
      # target_tls: true # if the app serves gRPC over TLS itself
```

- gRPC clients reach `grpc.example.com:443` over TLS with HTTP/2; `web.http2` must stay on, and the site must not be `tls: off` (validate notes both).
- `target_http2` speaks HTTP/2 to the app: h2c on a plain connection, or h2 inside `target_tls`, where the app must offer h2 in its TLS handshake. Trailers and `te: trailers` pass through, as gRPC needs.
- A health check with `path` suits websites, not gRPC apps; use `health_check: true` for them (validate notes it).

### gRPC streams

A long-lived gRPC stream is limited by burrow's timeouts like any request:

- The app must start its answer within `response_header_timeout_secs` (60 by default). A server stream that sends its headers at once is fine.
- A stream that makes no progress for 10 seconds is closed, so a stalled peer cannot hold memory. A window opened one byte at a time does not count as progress.
- A connection with nothing in flight is closed after `idle_timeout_secs`. An open stream keeps it alive; HTTP/2 PINGs alone do not.

Raise `response_header_timeout_secs` on the site for a method that thinks a long time before its first answer.

## Next steps

- [Health checks for websites](health-checks.md): use `health_check: true` for gRPC apps.
- [Security](../security.md#websites): the HTTP/2 limits burrow sets.
