# burrow docs

burrow makes apps on machines behind a router or firewall reachable through a public server: as plain ports, as websites with HTTPS, or privately for your own devices. These pages explain each part. If you are new, start with the [quick start in the README](../README.md#quick-start), then pick the page for what you want to do.

Words like client, visitor, service and target have one meaning throughout; the [glossary](glossary.md) lists them.

## Set up and expose things

| I want to… | Read |
| --- | --- |
| understand the three config files and every key | [Configuration](configuration.md) |
| expose a TCP or UDP port, limit who may connect, use several clients | [Services](services.md) |
| reach home machines from my laptop without opening them to the internet | [Private services and visitors](visitors.md) |
| connect clients through nginx, Caddy, Cloudflare or another HTTPS-only path | [WebSocket](websocket.md) |
| serve my apps as websites, with automatic HTTPS | [Websites](web.md) |

## Run and watch it

| I want to… | Read |
| --- | --- |
| start burrow at boot, update it, or run it in Docker | [Running as a service](service.md) |
| watch traffic and change the config in a browser | [burrow admin](admin.md) |
| let an AI app read or change the server | [burrow mcp](mcp.md) |
| see clients, services and certificates, or collect metrics | [Monitoring](monitoring.md) |
| know how burrow protects the tunnel and what I must do | [Security](security.md) |

## Websites in depth

| I want to… | Read |
| --- | --- |
| know what happens to a request, step by step | [How it works](web/how-it-works.md) |
| get certificates: Let's Encrypt, my own files, DNS checks, the local CA | [Certificates](web/certificates.md) |
| split one name between apps, or redirect | [Paths and redirects](web/routing.md) |
| add passwords, single sign-on, rate limits or device certificates | [Who may visit](web/access.md) |
| set headers, upload limits, error pages, or HTTPS to the app | [Headers, limits and pages](web/headers-limits-pages.md) |
| run behind Cloudflare, a load balancer or a home router | [Behind a proxy](web/proxies.md) |
| serve HTTP/2 or gRPC | [HTTP/2 and gRPC](web/http2-grpc.md) |
| check an app with a health page | [Health checks for websites](web/health-checks.md) |
| log every request | [Access log and request ids](web/logs.md) |
| let clients connect on port 443 or 80 | [The tunnel on the web ports](web/tunnel.md) |
| copy a ready config for a common app | [Recipes](web/recipes.md) |
| see every website key in one commented config | [A full example](web/full-example.md) |

## For contributors

| Page | What it holds |
| --- | --- |
| [protocol.md](protocol.md) | the wire protocol |
| [decisions.md](decisions.md) | why burrow is built the way it is |
| [research.md](research.md) | what burrow learned from rathole, frp, bore, chisel and rtun |
| [benchmark.md](benchmark.md) | the tunnel benchmark against rathole, frp and chisel, and how to reproduce it |
| [web-benchmark.md](web-benchmark.md) | the web proxy benchmark against nginx and Caddy, and how to reproduce it |
| [admin-ui-dev.md](admin-ui-dev.md) | working on the admin UI |
| [CONTRIBUTING.md](../CONTRIBUTING.md) | the code rules |
