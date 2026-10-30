# Behind a proxy, a load balancer or a home router

burrow does not have to face the internet directly. It can sit behind Cloudflare, Cloudflare Tunnel or a load balancer, or behind a home router that forwards ports to it. Two things then need care: burrow must learn each visitor's real address from the proxy, without believing anyone else who claims one, and Let's Encrypt's checks must still reach it. This page shows both.

In [How it works](how-it-works.md): the PROXY header is read in [step 1](how-it-works.md#1-a-connection-arrives-on-the-https-port), `X-Forwarded-*` is believed in [step 2](how-it-works.md#2-a-request-is-answered) and [step 6](how-it-works.md#6-port-80).

## Behind Cloudflare or a load balancer

Tell burrow which proxies are in front, and how each passes the visitor's address on. This config trusts Cloudflare's headers and a TCP load balancer's PROXY header:

```yaml
# server.yaml
web:
  https: 443
  letsencrypt:
    email: ops@example.com
  proxy_protocol_from: [10.0.0.5/32] # a TCP load balancer: it must send a PROXY header
  trusted_proxies: [173.245.48.0/20, 103.21.244.0/22] # Cloudflare's ranges
```

- **`trusted_proxies`** lists the HTTP proxies in front of burrow whose `X-Forwarded-For` and `X-Forwarded-Proto` are believed. burrow reads `X-Forwarded-For` from the right and takes the first address that is not a trusted proxy. From anyone else, these headers, `Forwarded` and every other `X-Forwarded-*` (also written with `_`) are removed before burrow sets its own. Cloudflare publishes its ranges at <https://www.cloudflare.com/ips/>; list them all.
- **Behind Cloudflare Tunnel (`cloudflared`)**, the proxy in front of burrow is `cloudflared` itself: `trusted_proxies` is its address, for example `127.0.0.1/32`, not Cloudflare's ranges.
- **`proxy_protocol_from`** lists TCP load balancers that start each connection with a PROXY header (v1 or v2). They must send it, and nobody else may. Their visitor's address is used from then on.
- The reported address is what `allow_from`, the access log and a service's own `proxy_protocol` see.
- **Let's Encrypt behind a proxy that does HTTPS:** the TLS check cannot pass through it. Use `challenge: http` (with `http: 80`) or `challenge: dns` on such sites; validate notes the sites that would use the TLS check.
- **A plain-HTTP site behind such a proxy** can still send `http://` visitors to `https://`: `https_redirect: true` on a `tls: off` site with `trusted_proxies` redirects by the proxy's `X-Forwarded-Proto`. If the proxy does not send it, every request redirects.

## Port forwarding at home

If the server is behind a home router, forward the internet's port 443 (and 80) to the same ports on the server. If the router forwards 443 to another port, like 8443, burrow cannot see that. Let's Encrypt checks a name only on port 443 or port 80 of the internet side, so with `web.https: 8443` validate asks you to forward port 80 to `web.http: 80` too, or to use your own certificate files:

```yaml
# server.yaml (the router forwards 443 to 8443 and 80 to 80)
web:
  https: 8443
  http: 80
  letsencrypt:
    email: ops@example.com
```

## Next steps

- [Nextcloud behind Cloudflare](recipes.md#nextcloud-behind-cloudflare), a complete recipe.
- [Certificates](certificates.md#dns-checks-wildcards-and-names-the-internet-cannot-reach): DNS checks, when no port can be reached from the internet.
