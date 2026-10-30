# How it works

This page follows a visitor through burrow, step by step, in the order the code runs. Read it when you want to know why a request got a certain answer, which check runs first, or which certificate a name gets. Each topic page links back to the step it changes. Nothing here needs to be set: it is what the keys on the other pages do, and when.

1. [A connection arrives on the HTTPS port](#1-a-connection-arrives-on-the-https-port)
2. [A request is answered](#2-a-request-is-answered)
3. [Which certificate a name gets](#3-which-certificate-a-name-gets)
4. [Let's Encrypt: checks and renewal](#4-lets-encrypt-checks-and-renewal)
5. [Saving a config](#5-saving-a-config)
6. [Port 80](#6-port-80)

## 1. A connection arrives on the HTTPS port

```
visitor ──TCP──> web.https
  │
  ├─ from web.proxy_protocol_from?  read the PROXY header: the visitor's own address
  ├─ too many connections without a name yet?  dropped
  ├─ read the TLS hello (10 s, 16 KiB): the name the visitor asks for (SNI)
  ├─ a Let's Encrypt TLS check?  answered, closed
  ├─ which site has this name?  none: closed
  │
  ├─ passthrough site ──────> the bytes go through the tunnel to the app as they are
  ├─ the tunnel's name ─────> TLS, then the tunnel's own handshake
  └─ website ───────────────> certificate (step 3), TLS, HTTP/1.1 or HTTP/2 (step 2)
```

1. **The visitor's address.** When the connection comes from an address in `web.proxy_protocol_from`, burrow first reads the PROXY header the balancer sends, and the address in it is the visitor from then on. A connection from there without a valid header within 10 seconds is closed. See [Behind Cloudflare or a load balancer](proxies.md#behind-cloudflare-or-a-load-balancer).
2. **The handshake budget.** Connections that have not shown a name yet are counted like the tunnel's own handshakes (`limits.max_handshakes`), on their own counter. When more than half of it is in use, one address (an IPv6 /64) cannot take more than its share. A connection over the limit is dropped before burrow reads anything.
3. **The name.** burrow reads the TLS hello, the first message of the connection: it has 10 seconds, and at most 16 KiB of it is read. Something that is not TLS is closed.
4. **Let's Encrypt's TLS check.** A hello that offers only `acme-tls/1`, for a name burrow is getting a certificate for, is answered with the check's own certificate and closed ([step 4](#4-lets-encrypt-checks-and-renewal)). This comes before anything about the site, so `allow_from` never blocks it.
5. **The site.** The name is looked up as it is written, then as the wildcard one label up: `x.apps.example.com` finds `*.apps.example.com` when no site has the exact name. A hello without a name (a visitor who typed an IP address) finds the site for the address it reached. An unknown name, or a plain-HTTP site's name (`tls: off`), is closed: without a certificate for it, burrow cannot show a page.
6. **What the site is.**
   - A **passthrough** site (`tls: passthrough`) gets the bytes as they came, the TLS hello included, through the tunnel ([Passthrough](../web.md#passthrough)).
   - The **tunnel's name**, or a name where only the tunnel's path is served, does TLS and then the tunnel's own handshake ([The tunnel on the same port](tunnel.md#the-tunnel-on-the-same-port)).
   - A **website** gets its certificate ([step 3](#3-which-certificate-a-name-gets)). With `client_auth` the device must show a certificate from its CA in the same handshake. The visitor and burrow then agree on HTTP/2 or HTTP/1.1 (`web.http2`, [HTTP/2](http2-grpc.md#http2)).
7. **Time.** All of this, from the first byte until burrow's TLS is done, has `limits.handshake_timeout_secs`. A connection that runs out of time is closed. A passthrough connection is on its own once the name is read: its TLS is the app's.

A connection stops counting against the handshake budget once burrow's TLS is done, or once a passthrough site's name is read.

## 2. A request is answered

```
request ──> headers, Host, path cleaned ──> route (longest path) ──> redirect
                                                  │
                                                  └─> service:
   allow_from ─> maintenance ─> rate_limit ─> basic_auth ─> forward_auth ─> max_body_size
        ─> app_path ─> through the tunnel to the app ─> response_headers, hsts, X-Request-Id, compress
        ─> access log
```

Each check below can stop the request. Before the app has started its answer, burrow answers with a page and a status code; after that, it can only close.

1. **The request itself.** More than 100 headers get `431`. A request without `Host`, or a `CONNECT`, gets `400`. On HTTPS, a `Host` other than the name of the connection gets `421`.
2. **The path is cleaned** once, and a `.` or `..` segment gets `400` ([How burrow reads a path](routing.md#how-burrow-reads-a-path)).
3. **The route.** The longest path of the name that matches wins, whatever the letter case ([Paths](routing.md#paths-several-apps-on-one-name)). No route: `404`. A redirect is answered at once ([Redirects](routing.md#redirects)). The tunnel's path on a shared name goes to the tunnel.
4. **Who asks.** The visitor's address is the connection's, or, from a proxy in `web.trusted_proxies`, the one in its `X-Forwarded-For` ([Behind Cloudflare or a load balancer](proxies.md#behind-cloudflare-or-a-load-balancer)). With `web.request_id`, the request gets its id here ([Access log and request ids](logs.md)).
5. **The site's checks, in this order:**
   1. `allow_from`: another address gets `403`.
   2. `maintenance: true`: `503` with the maintenance page.
   3. `rate_limit`: over the budget gets `429` with `Retry-After`.
   4. `basic_auth`: no or a wrong password gets `401`; too many wrong passwords from one address get `429`.
   5. `forward_auth`: the login service decides; its own answer goes back to the visitor when it says no.
   6. `max_body_size`: a body that says it is larger gets `413` at once; one that grows larger on the way is cut, and `413` is the answer.
6. **To the app.** `app_path` replaces the matched path ([Paths](routing.md#paths-several-apps-on-one-name)). burrow opens a connection to the app through the tunnel, or reuses one, and sends the request with `request_headers` and the `X-Forwarded-*` headers ([Headers, limits and pages](headers-limits-pages.md)). If it cannot:

   | Why | Answer |
   | --- | --- |
   | no client of the service is connected, or all of them are down | `503` |
   | the service is at its connection limit | `503` |
   | the client cannot reach the app, or the app's answer is broken | `502` |
   | the app has not sent its headers within `response_header_timeout_secs` | `504` |

7. **The answer.** `response_headers`, `hsts`, `X-Request-Id` and `compress` are applied to every answer of the site, the pages from burrow included. The access log writes its line when the answer has been sent ([Access log and request ids](logs.md)).

**Page or close.** Once the app has sent its headers, the status code has gone to the visitor. If the app or the tunnel fails after that, burrow cannot send a page any more: on HTTP/1.1 it closes the connection, and on HTTP/2 it ends only that stream (`RST_STREAM`), so the visitor's other requests on the connection go on. An HTTP/2 stream that makes no progress for 10 seconds is closed the same way ([gRPC streams](http2-grpc.md#grpc-streams)).

**The connection.** On HTTP/1.1 the connection is kept for the next request; a visitor has `web.request_header_timeout_secs` to send it. Any connection that moves no data for `web.idle_timeout_secs` is closed, and WebSockets stay open as long as they carry data ([Limits](headers-limits-pages.md#limits)).

## 3. Which certificate a name gets

```
name ──> the site's own cert/key? ──yes──> that file
           │ no
           ├─> tls: internal? ──yes──> the local CA signs one
           │ no
           ├─> listed in web.certificates? ──yes──> that file
           │ no
           ├─> under a shared wildcard in letsencrypt.dns? ──yes──> the wildcard
           │ no
           └─> Let's Encrypt, for this name
```

1. **The site's own file**, `tls: {cert, key}`, or the certificate uploaded for it. It must cover every name of the site.
2. **The local CA**, for a site with `tls: internal`: burrow signs a 12-hour certificate for its names itself ([The local CA](certificates.md#the-local-ca-tls-internal)).
3. **An entry of `web.certificates`** that lists the name, or a wildcard one label above it. A name listed in two entries has no certificate, and validate says so.
4. **A shared wildcard**: a name under a wildcard in `wildcards` of a `letsencrypt.dns` entry uses that one certificate, not one of its own.
5. **Let's Encrypt**, a certificate for the name, or for the site's names together ([step 4](#4-lets-encrypt-checks-and-renewal)).
6. Nothing of these: the name has no certificate. Validate refuses that config.

The tunnel's own name follows the same order, with `websocket.cert` and `websocket.key` as its own file. Names in `redirect_from` and `web.redirect_names` need a certificate like any other name, so they get one the same way. See [Certificates](certificates.md).

## 4. Let's Encrypt: checks and renewal

```
order ──> check (TLS on 443 / HTTP on 80 / DNS TXT) ──> certificate ──> in use
   ↑                                                                      │
   └──── renewal: at 2/3 of the life, or when Let's Encrypt says (ARI) ───┘
```

1. **Which check.** A site's `challenge` picks it. Without one, burrow tries the TLS check when `web.https` is 443, then the HTTP check when `web.http` is 80. Wildcards always use DNS, as do names with `challenge: dns` ([DNS checks](certificates.md#dns-checks-wildcards-and-names-the-internet-cannot-reach)). burrow remembers the check that worked for a name and tries it first next time.
2. **The answer.** The TLS check is answered when the connection arrives ([step 1](#1-a-connection-arrives-on-the-https-port)). The HTTP check is answered on port 80 before anything else ([step 6](#6-port-80)). For DNS, burrow sets the TXT record through the provider, waits until the record is seen, and removes it afterwards.
3. **Until the certificate is there**, an HTTPS handshake for the name fails, and `http://` gets a `503` page that says the certificate is on its way.
4. **Renewal** comes at two thirds of the certificate's life, and at half for short-lived ones (under 10 days). When Let's Encrypt suggests a time (ARI, RFC 9773), burrow follows it. The certificate in use stays until the new one is there.
5. **Failures** are tried again after 1 minute, 10 minutes, 100 minutes, then once a day. Short-lived certificates try every 6 hours, and in their last 2 days every 15 minutes to 4 hours. A rate limit from Let's Encrypt is waited out. burrow orders at most 4 new certificates for the same names in 7 days, under Let's Encrypt's limit of 5. See [When a certificate fails](certificates.md#when-a-certificate-fails).

## 5. Saving a config

```
save / SIGHUP ──> validate ──fail──> the running config stays
                     │ ok
                     └──> same ports? ──yes──> new names, routes and certificates at once
                              │ no
                              └──> bind the new ports, then let the old ones go
```

1. **Validate first.** The config is checked as `burrow validate` does. A config that does not pass is not applied, and the running one stays ([Reloading](../configuration.md#reloading)).
2. **Same ports.** When `web.https` and `web.http` stay the same, names, routes, certificates, limits and the access log change at once. Open connections stay open, and their next request uses the new settings: a name that is gone gets `404`.
3. **Other ports.** burrow binds the new ports and stops the old ones; when the new ports reuse an address of the old, the old stop first. A port that cannot be bound keeps the old ports, and the log says why.
4. **Files.** A certificate file that changes on disk is used from the next reload; a broken one keeps the last good certificate ([Your own files](certificates.md#your-own-files)). Config files are also checked every 30 seconds.
5. **Stopping.** On stop or a port change, a request in flight has 10 seconds to finish.

## 6. Port 80

```
request on web.http ──> Let's Encrypt HTTP check?   answered
          │
          ├─> the tunnel on port 80?                the tunnel
          ├─> plain site (tls: off)                 served (step 2)
          ├─> HTTPS site, https_redirect: false     served as plain HTTP (step 2)
          ├─> HTTPS site, from a trusted proxy that did HTTPS   served (step 2)
          ├─> HTTPS site, certificate ready         308 to https://
          ├─> HTTPS site, certificate on its way    503
          └─> passthrough, the tunnel's own name, unknown       404
```

1. **The request itself**, as on HTTPS: too many headers get `431`, no `Host` or `CONNECT` gets `400`. There is no TLS, so no `421`.
2. **Let's Encrypt's HTTP check.** A request for `/.well-known/acme-challenge/<token>` that burrow is waiting for is answered first, before any site, `allow_from` included.
3. **The path is cleaned**, as in [step 2](#2-a-request-is-answered).
4. **The tunnel on port 80** takes its path, or its whole name ([The tunnel on port 80](tunnel.md#the-tunnel-on-port-80-plain-ws-still-encrypted-by-burrow)).
5. **The name decides:**
   - a plain-HTTP site is served;
   - an HTTPS site with `https_redirect: false` is served as plain HTTP too;
   - when the request comes from a proxy in `web.trusted_proxies` with `X-Forwarded-Proto: https`, the proxy did HTTPS, and the site is served instead of redirected ([Behind Cloudflare or a load balancer](proxies.md#behind-cloudflare-or-a-load-balancer)). A plain site with `https_redirect: true` behind such a proxy is redirected to `https://` otherwise;
   - an HTTPS site redirects to `https://` with `308`, the path and query kept, and the port named when `web.https` is not 443; while its certificate is on its way it gets `503` instead, so a visitor is never sent to a name with no certificate yet;
   - a passthrough site, the tunnel's own name and an unknown name get `404`.
6. **Then the request goes on as in [step 2](#2-a-request-is-answered)**, from the route on.

## Next steps

- [Websites](../web.md): set up a first website.
- [A full example](full-example.md): every key in one config, and what validate checks.
