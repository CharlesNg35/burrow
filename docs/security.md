# Security

burrow carries traffic from the internet into your home or office network, so it lets in only what you allow. This page lists the few things you need to do yourself, then explains how burrow protects the tunnel, the websites and the admin.

## What you need to do

- **Use random secrets.** Make each secret with `burrow gensecret`. Never use a password.
- **Give each client and visitor its own secret.** burrow refuses two clients with the same secret.
- **Keep each client's `allow` list narrow.** List only the targets it should reach.
- **Keep config files private.** They hold secrets, so use `chmod 600 server.yaml`, or keep secrets in environment variables with `${VAR}`. `burrow validate` warns when a config is readable by every user.
- **Keep the metrics address on `127.0.0.1`,** unless a firewall limits who can connect.

## Secrets

A secret is 32 random bytes (256 bits). Someone who records a connection could try to guess the secret offline, as often as they like, so a secret must be random and long. burrow refuses anything that is not exactly 32 bytes, or does not look random.

**Changing a secret without downtime:**

1. On the server, list the old and the new secret: `secret: [old, new]` (up to 4).
2. Put the new secret in the client's config. It reconnects with it.
3. Remove the old secret from the server.

When you remove a client or visitor, or the secret it connected with, saving the server's config closes its open connections at once.

Secrets never appear in logs or error messages, only their length or a short fingerprint.

## Encryption

Every connection between client, server and visitor is encrypted and authenticated with the [Noise protocol framework](https://noiseprotocol.org/), the same framework WireGuard builds on. Each connection uses fresh keys, so a secret that leaks later does not reveal traffic recorded before (this is called forward secrecy).

The cipher is ChaCha20-Poly1305 by default. `cipher: aes-gcm` in the client or visitor config uses AES-GCM instead, which is faster on CPUs with AES instructions.

`insecure_plaintext: true` sends the data of forwarded TCP connections unencrypted between the server and a client that connects over plain TCP, for example inside a network you trust completely. It must be set on both the server and the client. Everything else stays encrypted: logging in still uses the full handshake, so the secret never crosses the network in clear text, and WebSocket links stay encrypted.

[protocol.md](protocol.md) describes the exact protocol, and the point where each side is proven.

## Who can reach what

Several checks decide what a connection may reach, and the client has the last word:

- **The client decides what it connects to.** A client connects only to targets in its own `allow` list, checked after host names are resolved. So even someone who takes over the server, or a visitor with `reach`, cannot use the client to reach other machines.
- **A private service has no port on the server at all.** Only the visitors it lists can reach it.
- **A visitor reaches only** the services that list it, and clients only through `reach`.
- **`allow_from` and `deny_from`** are checked on the server before anything enters the tunnel, also for visitors of private services.
- **The server itself** (`client: self`) connects only to what `self.allow` permits. It never connects to its own listen, WebSocket or metrics addresses, nor to link-local or cloud metadata addresses that `self.allow` does not name on their own.

## Guessing names

Wrong names and wrong secrets get exactly the same answer, after the same amount of work, so nobody can find out which client or visitor names exist. Connections that have not logged in yet are limited in number and time (`limits.max_handshakes`, `limits.handshake_timeout_secs`). When they run short, one address cannot take more than its share.

<details>
<summary><b>More details</b></summary>

- A visitor that comes through a proxy in front of `websocket.listen` has the proxy's address, and `validate` warns where a service's `allow_from` or `deny_from` relies on it.
- Unix socket targets need a `unix:` entry in the client's `allow` list. A socket path that is a symbolic link is refused.
- The lock that stops the same tunnel from running twice lives in a folder every local user can write to, so it also catches copies started by hand. A lock held by another user's process is ignored with a warning, so nobody can keep a service from starting that way.
- An installed service runs as its own account without login or home folder, from its own copy of the binary and config, and on Linux in a sandbox ([service.md](service.md)).
- `burrow update` checks every download against the release's `SHA256SUMS` before it is used.

</details>

## Websites

A website is open to every browser, so burrow checks each request before it reaches the app:

- **HTTPS ends at the server** for websites burrow serves with `tls: auto` or its own files. The server decrypts the request, and it travels to the client inside the tunnel's own encryption. With `tls: passthrough`, the server never sees the content, and the app keeps its own certificate.
- **Plain HTTP sites are unencrypted** between the visitor and the server. `burrow validate` and the admin UI mark each one; keep them on networks you trust.
- **The app learns the visitor's address** from `X-Forwarded-For` and `X-Real-IP`, which burrow sets and replaces, so a visitor cannot forge them.
- **Paths are cleaned before a route is picked.** Repeated slashes are merged, plain escapes decoded and the rest written in capitals. `.` or `..` segments are refused with 400, also when encoded or hidden behind an encoded slash. So a password or other rule on `/admin` cannot be walked around with `//admin`, `/%61dmin` or `/public%2F..%2Fadmin`. The app gets the cleaned path ([How burrow reads a path](web/routing.md#how-burrow-reads-a-path)).
- **Forwarded headers are believed only from `web.trusted_proxies`.** From anyone else, burrow removes `Forwarded` and every `X-Forwarded-*` header (also written with `_`) before it sets its own. It reads `X-Forwarded-For` from the right, so a visitor cannot put an address in front of the proxy's. A `Connection` header can never remove `Host` or the headers burrow sets.
- **PROXY headers only from `web.proxy_protocol_from`.** Listed load balancers must send one. From anyone else, the bytes are read as TLS or HTTP, so a visitor cannot claim another address. `validate` warns when either list holds the whole internet.
- **HTTP/2 limits** are set by burrow, not left to library defaults. They answer the HTTP/2 attacks of recent years (CVE-2023-44487, CVE-2024-27316, CVE-2025-8671, the 2026 HTTP/2 Bomb):
  - at most 100 header fields per request (cookie pieces counted; 431 above) and 32 KiB of decoded headers;
  - a cap on concurrent streams, and on resets a client may cause (Rapid Reset, MadeYouReset);
  - CONTINUATION floods refused;
  - a stream that makes no progress for 10 seconds closed;
  - the buffers per connection kept to about 8 MiB.
- **Logins never fail open** ([Who may visit](web/access.md)):
  - `basic_auth` takes argon2id or bcrypt hashes with capped costs, and never passes the `Authorization` header to the app. It allows 10 wrong passwords a minute per address (IPv6 per /64), on all sites together, and then answers `429`, the right password included.
  - `forward_auth` removes a visitor's own copies of the headers every login service passes on (also written with `_`). A login service that is down or slow gives `502`, `503` or `504`, never the app.
  - `client_auth` checks device certificates in the TLS handshake, and refuses the devices in its revocation list. It answers `421` to a request for another name than the connection's, so a wildcard certificate cannot carry a visitor from an open name to a protected one.
- **`target_tls` checks the app's certificate** unless you write `insecure_skip_verify: true`, which validate notes.
- **DNS provider secrets** (`cloudflare.token`, `rfc2136.secret`, `acme_dns.password`) come only from `${VAR}` or `file:`; validate refuses one written in the config. They are never printed, and burrow wipes them from memory when they are no longer needed. Give a Cloudflare token only Zone: Read and DNS: Edit on the zones it serves, and an RFC 2136 key only the `_acme-challenge` names.
- **A DNS `command` hook** gets no secrets and only `PATH`; validate warns when others can change it.
- **Certificates and keys** from Let's Encrypt live in a folder only the service account can read (0700, files 0600). Your own key files must be readable by the account burrow runs as.
- **The admin UI cannot read files or run programs.** `web.proxy_protocol_from`, every certificate, key and CA path, `error_pages`, `access_log`, and a DNS provider's `command`, secrets and server are changed in server.yaml only ([web.md](web.md)).
  - A write admin may set the web proxy's ports (`web.https`, `web.http`), as with a service's public `bind`. The review names each port it opens and the websites a removal stops. A port shared with the tunnel stays a file edit, and a port another program holds is refused before anything is written.
  - Instead of a file path, a write admin may upload a certificate, CA or revocation list. burrow checks it, keeps it private in its state folder, and never sends a key back ([Uploading files](web/certificates.md#uploading-files-in-the-admin-ui)).
- **The local CA (`tls: internal`)** may sign only LAN names and addresses (X.509 name constraints in its root). Its key stays in the state folder, readable only by the service account, and the admin UI offers its certificate, never the key.

## burrow admin

The web admin listens on `127.0.0.1` only, needs a random token that only you can read, and can lower admin access but never grant or widen it. See [admin.md](admin.md#security-notes).

## burrow mcp

`burrow mcp` gives an AI app the same admin channel, with the same server rules. It is read-only unless started with `--allow-write` by a visitor with `admin: write`. It applies only a plan the AI checked first, and secrets never reach the AI. Over HTTP, it listens on `127.0.0.1` only, needs a bearer token kept in a file only you can read, and refuses requests from web pages ([mcp.md](mcp.md#safety)).

## Next steps

- [The wire protocol](protocol.md): the handshake and encryption in detail.
- [Why burrow is built this way](decisions.md).
