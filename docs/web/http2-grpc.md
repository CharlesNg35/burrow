# HTTP/2 and gRPC

## HTTP/2

Browsers use HTTP/2 on `web.https` when they can, which loads pages with many parts faster; others keep HTTP/1.1. burrow talks to the app over HTTP/1.1 either way, unless the site says `target_http2`. `web.http2: false` keeps every site on HTTP/1.1, for a client that breaks with HTTP/2. `burrow validate` shows which is on: `web: https [::]:443 (HTTP/2 and HTTP/1.1, IPv4 and IPv6)`.

- HTTP/2 needs HTTPS: plain-HTTP sites and port 80 speak HTTP/1.1 only.
- WebSockets keep working: browsers open them over HTTP/1.1 beside the HTTP/2 connection.
- burrow sets every HTTP/2 limit itself, against the attacks of recent years (see [security.md](../security.md#websites)).

## gRPC

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

- the app must start its answer within `response_header_timeout_secs` (60 by default); a server stream that sends its headers at once is fine;
- a stream that makes no progress for 10 seconds is closed, which keeps a stalled peer from holding memory; a window opened one byte at a time does not count as progress;
- a connection with nothing in flight is closed after `idle_timeout_secs`; an open stream keeps it alive, HTTP/2 PINGs alone do not.

Raise `response_header_timeout_secs` on the site for a method that thinks a long time before its first answer.
