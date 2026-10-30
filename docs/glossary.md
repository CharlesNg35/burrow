# Glossary

The words that burrow's docs, its messages and the admin UI use, each with one meaning throughout.

## The tunnel

| Term | Meaning |
| --- | --- |
| **server** | The burrow machine with a public address, usually a small cloud server. It reads `server.yaml`, which decides everything that is exposed. |
| **client** | A burrow machine behind a router or firewall, usually at home or in an office. It connects out to the server and reaches your apps for it. Its config is `client.yaml`. |
| **visitor** | One of your own devices, such as a laptop, that connects to the server with its own name and secret to use private services. Its config is `visitor.yaml`, and it runs `burrow visit`. |
| **tunnel** | The encrypted connections between a client (or visitor) and the server. Everything a service carries travels inside it. |
| **channel** | One encrypted connection inside the tunnel. A client keeps a few idle channels open in advance (the pool; burrow admin calls them ready connections), so a new connection starts at once. |
| **secret** | 32 random bytes that a client or visitor proves it knows to log in. Made with `burrow gensecret`; never a password. |
| **NAT** | Network address translation: what a home router does, so machines behind it can connect out but cannot be reached from the internet. burrow exists to get past it. |

## Services

| Term | Meaning |
| --- | --- |
| **service** | One thing you make reachable, leading to a target: a public port on the server, a website, private access for visitors, or any mix of these. Written under `services:` in `server.yaml`. |
| **target** | Where the client connects for a service, written as the client sees it: `127.0.0.1` is the client machine itself. A port alone means `127.0.0.1` on the client. |
| **bind** | The address and port the server opens for a service. A service without `bind` opens no port. |
| **public service** | A service with a `bind` port, open to anyone who can reach the server, unless `allow_from` limits it. |
| **private service** | A service without `bind`: only the visitors it lists can reach it. |
| **`self`** | The server itself used as a client: `client: self` runs a service on the server's own machine or network. |
| **`allow` list** | The client's own list of targets it agrees to connect to. It has the final say; nothing else is reached. |
| **reach** | A visitor's permission to pick its own targets on a client, like `ssh -L`. |
| **forward** | One local port a visitor opens on its own machine, leading to a service or a target. |
| **failover, balance** | How a service with several clients picks one: the first one that is up, or spread over all of them. |
| **health check** | The server asking through each client, every few seconds, whether the target answers. |

## Websites

| Term | Meaning |
| --- | --- |
| **web proxy** | burrow's built-in reverse proxy, turned on by a top-level `web:` block. It serves services as websites by name. |
| **website**, **site** | A service with its own `web:`: browsers reach it by a name, like `blog.example.com`, on the server's HTTPS or HTTP port. |
| **host name**, **name** | The address a browser asks for, like `blog.example.com`. One name leads to one site (or one site per path). |
| **wildcard** | A name like `*.example.com` that covers every name exactly one label below it, like `blog.example.com`. |
| **certificate** | The file that lets a browser trust an HTTPS site. burrow gets them from Let's Encrypt, from your files, or from its local CA. |
| **Let's Encrypt** | A free, automatic certificate authority. burrow orders and renews its certificates for you once `web.letsencrypt` has an email. |
| **check** (Let's Encrypt) | How Let's Encrypt confirms you control a name before it issues a certificate: the **TLS check** on port 443, the **HTTP check** on port 80, or the **DNS check**, a TXT record burrow sets through your DNS provider. The ACME standard calls them challenges, hence the `challenge` key. |
| **shared wildcard** | One Let's Encrypt wildcard certificate, listed under `letsencrypt.dns[].wildcards`, that every site under it reuses. |
| **local CA** | burrow's own certificate authority, for names that live only on your network (`tls: internal`). You install its certificate once on each device. |
| **passthrough** | `tls: passthrough`: burrow reads only the name and passes the encrypted connection to an app that does its own HTTPS. |
| **plain HTTP site** | `tls: off`: a site served without HTTPS, on `web.http`. Its traffic is not encrypted. |
| **trusted proxy** | An HTTP proxy in front of burrow, such as Cloudflare, whose `X-Forwarded-For` header burrow believes (`web.trusted_proxies`). |
| **state folder** | The private folder where burrow keeps its Let's Encrypt account and certificates, the local CA and uploaded files. `burrow status` names it. |

## Running and managing

| Term | Meaning |
| --- | --- |
| **admin visitor** | A visitor with `admin: read` or `admin: write`, used by `burrow admin` and `burrow mcp`. |
| **burrow admin** | The web admin: a page on your own machine to watch traffic and change the server's config, with a review before each change. |
| **reload** | Applying a saved config without a restart. burrow reloads by itself when the file changes. |
| **validate** | `burrow validate`: checks a config without starting anything and explains every problem with a fix. |
| **installed service** | burrow set up with `burrow service install`, so the system starts it at boot and restarts it after a failure. |

## Next steps

- [Start with the README](../README.md) for a five-minute setup.
- [All docs, by task](README.md).
