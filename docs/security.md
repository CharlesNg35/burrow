# Security

This page explains how burrow protects your tunnel, and what you need to do.

## What you need to do

- **Use random secrets.** Make each secret with `burrow gensecret`. Never use a password.
- **Give each client and visitor its own secret.** burrow refuses two clients with the same secret.
- **Keep each client's `allow` list narrow.** List only the targets it should reach.
- **Keep config files private.** They hold secrets. Use `chmod 600 server.yaml`; `burrow validate` warns when a config is readable by every user. Or keep secrets in environment variables with `${VAR}`.
- **Keep the metrics address on `127.0.0.1`,** unless a firewall limits who can connect.

## Secrets

A secret is 32 random bytes (256 bits). Someone who records a connection could try to guess the secret offline, as many times as they like, so a secret must be random and long. burrow refuses anything that is not exactly 32 bytes, and anything that does not look random.

**Changing a secret without downtime:**

1. On the server, list the old and the new secret: `secret: [old, new]` (up to 4).
2. Put the new secret in the client's config. It reconnects with it.
3. Remove the old secret from the server.

Removing a client or visitor, or the secret it connected with, and saving the server's config closes its open connections at once.

Secrets never appear in logs or error messages, only their length or a short fingerprint.

## Encryption

Every connection between client, server and visitor is encrypted and authenticated with the [Noise protocol framework](https://noiseprotocol.org/), the same framework WireGuard builds on. Each connection uses fresh keys, so a secret that leaks later does not reveal traffic recorded before (this is called forward secrecy).

The cipher is ChaCha20-Poly1305 by default. `cipher: aes-gcm` in the client or visitor config uses AES-GCM instead, which is faster on CPUs with AES instructions.

`insecure_plaintext: true` sends the data of forwarded TCP connections unencrypted between the server and a client that connects over plain TCP, for example inside a network you trust completely. Everything else stays encrypted: logging in still uses the full handshake, so the secret never crosses the network in clear text, and WebSocket links stay encrypted. It must be set on both the server and the client.

The exact protocol, and the point where each side is proven, is described in [protocol.md](protocol.md).

## Who can reach what

- **The client decides what it connects to.** A client only connects to targets in its own `allow` list, checked after host names are resolved. So even someone who takes over the server, or a visitor with `reach`, cannot use the client to reach other machines.
- **A private service has no port on the server at all.** Only the visitors it lists can reach it.
- **A visitor reaches only** the services that list it, and clients only through `reach`.
- **`allow_from` and `deny_from`** are checked on the server before anything enters the tunnel, also for visitors of private services.
- **The server itself** (`client: self`) only connects to what `self.allow` permits. It never connects to its own listen, WebSocket or metrics addresses, nor to link-local or cloud metadata addresses that `self.allow` does not name on their own.

## Guessing names

Wrong names and wrong secrets get exactly the same answer, after the same amount of work, so nobody can find out which client or visitor names exist. Connections that have not logged in yet are limited in number and time (`limits.max_handshakes`, `limits.handshake_timeout_secs`), and one address cannot take more than its share when they run short.

<details>
<summary><b>More details</b></summary>

- A visitor that comes through a proxy in front of `websocket.listen` has the proxy's address, and `validate` warns where a service's `allow_from` or `deny_from` relies on it.
- Unix socket targets need a `unix:` entry in the client's `allow` list. A socket path that is a symbolic link is refused.
- The lock that stops the same tunnel from running twice lives in a folder every local user can write to, so it also catches copies started by hand. A lock held by another user's process is ignored with a warning, so nobody can keep a service from starting that way.
- An installed service runs as its own account without login or home folder, from its own copy of the binary and config, and on Linux in a sandbox. See [service.md](service.md).
- `burrow update` checks every download against the release's `SHA256SUMS` before it is used.

</details>

## Websites

- **HTTPS ends at the server** for websites burrow serves with `tls: auto` or its own files: the server decrypts the request, and it travels to the client inside the tunnel's own encryption. With `tls: passthrough` the server never sees the content; the app keeps its own certificate.
- **Plain HTTP sites are unencrypted** between the visitor and the server. `burrow validate` and the admin UI mark each one; keep them on networks you trust.
- **The app learns the visitor's address** from `X-Forwarded-For` and `X-Real-IP`, which burrow sets and replaces, so a visitor cannot forge them.
- **Paths are cleaned before a route is picked:** repeated slashes merged, plain escapes decoded and the rest written in capitals, and `.` or `..` segments refused with 400, also encoded or hidden behind an encoded slash. A password or other rule on `/admin` cannot be walked around with `//admin`, `/%61dmin` or `/public%2F..%2Fadmin`. The app gets the cleaned path. See [web.md](web/routing.md#how-burrow-reads-a-path).
- **Forwarded headers are believed only from `web.trusted_proxies`.** From anyone else, burrow removes `Forwarded` and every `X-Forwarded-*` header (also written with `_`) before it sets its own. It reads `X-Forwarded-For` from the right, so a visitor cannot put an address in front of the proxy's. A `Connection` header can never remove `Host` or the headers burrow sets.
- **PROXY headers only from `web.proxy_protocol_from`:** listed load balancers must send one, and from anyone else the bytes are read as TLS or HTTP, so a visitor cannot claim another address. `validate` warns when either list holds the whole internet.
- **HTTP/2 limits** are set by burrow, not left to library defaults: at most 100 header fields per request (cookie pieces counted; 431 above) and 32 KiB of decoded headers, a cap on concurrent streams and on resets a client may cause (Rapid Reset, MadeYouReset), CONTINUATION floods refused, a stream that makes no progress for 10 seconds closed, and the buffers per connection kept to about 8 MiB. They answer the HTTP/2 attacks of recent years (CVE-2023-44487, CVE-2024-27316, CVE-2025-8671, the 2026 HTTP/2 Bomb).
- **`target_tls` checks the app's certificate** unless you write `insecure_skip_verify: true`, which validate notes.
- **DNS provider secrets** (`cloudflare.token`, `rfc2136.secret`, `acme_dns.password`) come only from `${VAR}` or `file:`; validate refuses one written in the config. They are never printed, and burrow wipes them from memory when they are no longer needed. Give a Cloudflare token only Zone: Read and DNS: Edit on the zones it serves, and an RFC 2136 key only the `_acme-challenge` names. A `command` hook gets no secrets and only `PATH`; validate warns when others can change it.
- **Certificates and keys** from Let's Encrypt live in a folder only the service account can read (0700, files 0600). Your own key files must be readable by the account burrow runs as.
- **The admin UI cannot open ports, read files or run programs:** `web.https`, `web.http`, `web.proxy_protocol_from`, every certificate, key and CA path, `error_pages`, `access_log`, and a DNS provider's `command`, secrets and server are changed in server.yaml only. See [web.md](web.md).

## burrow admin

The web admin listens on `127.0.0.1` only, needs a random token that only you can read, and can lower admin access but never grant or widen it. See [admin.md](admin.md#security-notes).
