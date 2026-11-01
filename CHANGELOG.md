# Changelog

## [0.2.1]

- **Built-in login (OIDC):** `web.oidc` providers like Google or Keycloak, picked per site with `oidc: <name>` and `allow_users`. See docs/web/access.md.
- **Countries:** `allow_countries` and `deny_countries` per service, for public ports, websites and UDP. The free DB-IP Lite database is downloaded and kept up to date on its own, or `geoip:` names your own `.mmdb` file. See docs/services.md#by-country.
- **Notifications:** `notify:` sends a message to a webhook, an ntfy topic or an email address when a client goes offline, a certificate fails or a service's health check fails, and once more when it is fixed. Off unless set; see docs/monitoring.md.

## [0.2.0]

Websites with automatic HTTPS, a reworked admin UI, load balancing with weights, and `burrow mcp` for AI apps. Existing configs work unchanged, and v0.1.x clients and visitors keep working with a v0.2.0 server.

- **Websites (the web proxy):** many websites on one port 443, told apart by their names, through the tunnel to the app at home. It is off until a top-level `web:` is written, and a service becomes a site with `web: blog.example.com`. See docs/web.md.
  - HTTPS done by burrow, passed through to an app that does its own (`tls: passthrough`, like a NAS), or plain HTTP (`tls: off`). `http://` visitors are sent to `https://` by default.
  - A bare port listens on IPv4 and IPv6. `burrow validate` prints every name, where it leads and where its certificate comes from.
- **Certificates:**
  - Let's Encrypt, opt-in with `web.letsencrypt.email`: the TLS check on 443, the HTTP check on 80, or the DNS check. Renewal follows Let's Encrypt's suggested time (ARI). Profiles are supported, and a server with no domain gets short-lived certificates for its IP address.
  - DNS checks through Cloudflare, RFC 2136 (BIND, Knot, PowerDNS, Technitium), acme-dns or a command of your own, for wildcards and names the internet cannot reach. Several providers can be set by domain, `_acme-challenge` CNAMEs are followed, and one shared wildcard can cover many sites.
  - Your own certificate files, or certificates uploaded in the admin UI. A file renewed on disk is picked up on the next reload.
  - A local CA for LAN names (`tls: internal`), like Caddy's. Install it once per device; `burrow ca export` prints its certificate.
  - A renewal order that the CA refuses twice with its `replaces` hint (ARI) is sent without the hint on the third try, so a certificate never expires over the hint.
  - Certificates and the Let's Encrypt account live in a private state folder (`--state-dir`). `service uninstall` keeps it; `--purge` removes it.
- **Who may visit:**
  - `basic_auth` with argon2id or bcrypt hashes, from the new `burrow passwd` or set in the admin UI. Wrong passwords are limited to 10 a minute per address.
  - Named login services in `web.forward_auth`, such as Authelia, authentik or oauth2-proxy, picked per site with `forward_auth: <name>`.
  - Device certificates (`client_auth`), with revocation lists.
  - `rate_limit` per visitor address, and the service's `allow_from` and `deny_from`.
- **Paths, redirects and pages:** `paths` and `app_path` split one name between apps. `redirect_from`, `redirects` and `web.redirect_names` handle redirects. Sites also get request and response headers, `max_body_size`, gzip (`compress`), HSTS, error pages, a maintenance page, HTTPS to the app (`target_tls`), an access log, request ids and timeouts (`web.request_header_timeout_secs`, `web.idle_timeout_secs`, `response_header_timeout_secs`).
- **Behind a proxy:** `web.trusted_proxies` reads the visitor's address from `X-Forwarded-For` (Cloudflare, load balancers). `web.proxy_protocol_from` accepts PROXY headers from TCP balancers.
- **HTTP/2 and gRPC:** HTTP/2 to browsers is on by default, and `target_http2` speaks HTTP/2 to the app for gRPC. burrow sets its own limits against the HTTP/2 attacks of recent years (Rapid Reset, MadeYouReset, CONTINUATION floods, the 2026 HTTP/2 Bomb). Paths are cleaned before routing, so `//admin` or `/%61dmin` cannot walk around a login.
- **Health checks for websites:** `health_check: { path: /healthz }` asks each client's app for a page and sends visitors only to clients whose app answers.
- **The tunnel on the web ports:** clients can connect over `wss://` on port 443 (or `ws://` on 80), on a name of their own or a path of a site, with an automatic certificate. `burrow init server --ws --email ADDRESS --server NAME` writes this setup.
- **Post-quantum TLS:** burrow's HTTPS, the tunnel's `wss://` and HTTPS to the app prefer the hybrid key exchange X25519MLKEM768, as browsers, Go and OpenSSL 3.5 do. A client that does not offer it gets X25519, P-256 or P-384 as before.
- **Load balancing:**
  - `mode: least_connections` joins `failover` and `balance`.
  - With `balance`, a client can take a weight, `{name: home, weight: 3}` (smooth weighted round robin, as in nginx).
  - A `clients` entry may give its own `target`, so one client in front of several machines can spread a service over them. Health checks and open counts are kept per entry. The client gets the target as before, so older clients work.
- **`burrow mcp`:** lets Claude Code, Codex, Cursor or Claude Desktop read a running server over MCP, on stdio or loopback HTTP with a token.
  - With `--allow-write` and a write admin, it can also change the config, but only through a checked plan that you approve. Secrets never reach the AI.
  - `--print-config` prints a ready setup for each app. See docs/mcp.md.
- **burrow admin, reworked:**
  - **Overview:** tabs for Services, Clients, Visitors, Websites and Certificates. Clients that are gone read as offline, and refused connections are counted with their reason.
  - **Reached by:** a service page says how the service is reached, with three switches: a public port, a website and visitors, in any mix. Review says what turning one off removes.
  - **Settings:** four tabs (Server, Connections, Web, Monitoring) under one search. The web ports, WebSocket, login services, names that only redirect and the local CA can be changed there.
  - **Every setting is visible.** Rarely used ones sit under "More settings". Ones that do not apply are shown disabled, with the reason. Fields that only server.yaml can change say so.
  - **Websites and certificates:**
    - "New website" turns the web proxy on when needed.
    - Certificates, CAs and revocation lists can be uploaded instead of naming files.
    - Passwords are hashed by the server, and a key or password is never shown again.
  - **Review and Activity:** Review shows the changed lines of server.yaml and plain-word change titles. Versions are named by the time they were applied, and you can go back to any of them.
  - **Layout:** section cards, a table for client entries, and pages that work on a phone and in dark mode.
- **Mixed versions:** a v0.2.0 admin works with an older server and leaves out what it cannot do. An older admin can open a v0.2.0 server: Needs attention lists the settings it cannot edit and shows them as text. Editing stays allowed, because the server checks every change.
- **burrow status and metrics:** `burrow status` lists websites, certificates (source, state, expiry, next renewal, why one fails) and the order queue, and shows why a client's last health check failed, the local CA's fingerprint, and a web port that waits for its address. New metrics: `burrow_web_responses_total`, `burrow_certificate_expiry_timestamp_seconds` and `burrow_certificate_failures_total`.
- **New commands and flags:**
  - `burrow passwd` and `burrow ca export`;
  - `burrow init server --web NAME --email ADDRESS` (with `--web-client` and `--web-target`), and a last question in `init server` about serving websites, no by default;
  - `--state-dir` for a server;
  - `service uninstall --purge` also removes the web state folder and access log.
- **Running as a service:**
  - A server with `web:` gets its private state folder at install.
  - A start that fails only because a web certificate, key or CA file cannot be read exits for a retry.
- **Docs:** a new docs index by task, a glossary, a page per web topic with recipes (Home Assistant, Nextcloud, Grafana, authentik and more), a full commented example, and admin UI screenshots in the README.
- **Under load:**
  - New connections to the web ports wait for a free handshake slot within their handshake time instead of being dropped, and `limits.max_handshakes` is now 2048 (was 256).
  - Bursts to a client wait instead of failing: past `limits.max_pending` (512) connections waiting for a channel, up to three times as many more wait in line for their usual 10 s instead of getting busy at once, with a fair part of the line per website visitor address. v0.1.x clients get this too.
  - When a service's or client's connections run short, one website visitor address gets at most a fair part (an eighth), so one address cannot take a site away from everyone else.
  - Returning visitors resume their TLS session with session tickets, except on sites with `client_auth`.
  - The web proxy keeps small, reused buffers per connection, so it uses less than half the memory and about 30 % less CPU per request with large bodies.
  - The Linux amd64, arm64 and riscv64 builds give freed memory back to the system within seconds of going idle.
  - Public UDP ports and UDP forwards ask the system for a 7 MiB receive buffer, as WireGuard does, and each UDP peer for 256 KiB, so a burst of datagrams waits instead of being dropped. Linux caps this at `net.core.rmem_max` (see docs/services.md).
- **Compatibility:**
  - The tunnel protocol is unchanged: v0.1.x clients and visitors work with a v0.2.0 server, and v0.2.0 clients and visitors with a v0.1.x server.
  - The admin protocol adds features that each side uses only when the other lists them.
  - Nothing in an existing config changes meaning. Everything new is off until written, except the load handling above and the new `limits.max_handshakes` default.

## [0.1.2]

UDP for visitors, and limits you can set and see.

- **Visitors reach UDP services:**
  - A private UDP service may list `visitors`, and a forward with `protocol: udp` (or `burrow visit --udp`) carries its datagrams, port ranges included.
  - `reach` now covers UDP as well as TCP for the same addresses and ports, so an existing `reach` grant also allows UDP to them; the client's `allow` list still decides.
  - `init visitor`, `--update`, `visitor-config` and the admin UI's Connect tab write `protocol: udp` forwards with free UDP ports; a forward whose protocol does not match its service is told which one to set.
  - Needs a v0.1.2 server; against an older one, `init visitor` lists TCP services only and keeps your UDP forwards. Clients need no update.
- **The SOCKS5 forward carries UDP** (UDP ASSOCIATE, such as DNS through the proxy). It takes datagrams only from the app that asked, drops fragments, checks each destination against `reach` and `allow`, and ends with the app's TCP connection.
- **burrow admin:** a private UDP service can be added once it lists visitors, and the visitor page tags its UDP services.
- **burrow admin help and pickers:**
  - Every field has help, and every address, list, path and rate field shows one short example, such as `certs/fullchain.pem` for a TLS certificate.
  - In a picker, "New visitor" (or client, or service) keeps the list open with the name box focused, so you can type the name and press Enter.
  - A TLS key file path is shown as written instead of being masked as a secret.
  - A locked address field keeps its help while editing.
- **An older server shows in Needs attention** in `burrow admin`, with how to update it, and About marks its version.

- **`max_connections` in client.yaml** (default 16384, 1 to 1000000): the most connections open through a client at once. A saved change applies at once without a reconnect, and open connections keep running when it is lowered. If the open file limit is too small for it, the client lowers it, with one warning at start and after each change when you set the value yourself.
- **UDP peer limits:** `max_udp_peers` in client.yaml (default 8192) and `udp.max_peers` in server.yaml (default 4096 per service port, editable in the admin UI), 1 to 1000000, applied at once like `max_connections`; the client's `max_udp_peers` is also lowered to fit the open file limit. A client now follows the server's per-port limit instead of its own fixed 4096.
- **Seeing a limit:** a client at its `max_connections` logs at most one warning a minute with the number refused since the last one, the server's log points to the client's `max_connections`, `burrow status` and the admin UI mark the client "at limit", the service's REFUSED count includes those refusals, and `burrow_client_busy_total` counts them per client.
- **UDP peers are kept 2 minutes** without traffic by default (`udp.session_timeout_secs`, was 60 s) on the server, the client and the visitor, the least RFC 4787 asks of a NAT, so quiet UDP flows such as games and VPNs keep their mapping.
- **Running at scale** in docs/monitoring.md: memory per connection, the limits to raise, and the Linux limits that run out first.
- The `burrow admin` note for a moved port says "taken" only when the port is in use, gives the real error otherwise, and says "open the new link" only when a link is shown.
- `sudo burrow admin` keeps its admin session in root's own home instead of the user's, so later runs without sudo keep their session, and sudo runs still reconnect their open tab.
- An open admin tab keeps retrying every 2 s for 5 minutes after `burrow admin` stops (was 30 s), so a restarted `burrow admin` reconnects it instead of opening a second tab.
- A service install that takes over an older unit gives its old config and record files back to root, and uninstall does the same for older records, before the service account is removed, so no file stays owned by an account that no longer exists.
- A UDP datagram too large for one tunnel record is dropped with a debug log line instead of silently.
- The admin UI names the limit a client hit (`max_connections` in client.yaml), and calls the server's per-client field "Max connections on this server".
- `burrow admin --url-file` says to open the new link when its port moved.
- The last `burrow service uninstall` removes burrow's own service account also after an install found it left behind by an earlier install, such as after taking over a stopped unit; it is still kept while another service or unit uses it.

## [0.1.1]

Stability and usability fixes.

- **Service management:**
  - A burrow service that this burrow has no usable record of, installed by an older layout or by hand, is no longer invisible. `burrow service list`, `status` and `update` name it with the commands that take it over, and a command without a name asks which service it means.
  - `service install` refuses to replace such a service while it runs, never reports a service it did not restart as started, and names the service holding a busy port.
  - Uninstalling the last managed service keeps the binary, the service account and the runtime folder setup while such a service still uses them.
  - `burrow install`, `uninstall`, `start`, `stop`, `restart` and `logs` point to `burrow service ...`.
- **`burrow admin`:**
  - The session (token, path and port) is kept for 12 hours per visitor config in a file only you can read, so an open tab reconnects after a restart and no second tab opens.
  - If the last session's port is taken, it says so and names the port it moved to.
  - A tab whose session ended says so instead of retrying forever, and the first load retries a busy server.
- **Admin UI:**
  - Help gives example values again.
  - The overview counts, traffic readout, client details, TCP/UDP tags and open counts are back.
  - Port fields show what is saved, settings show their full key, and a review marks what a path was before.
  - The Connect tab links to the download and uses `./burrow`.
  - A UDP service can clear values it does not use, and the reach editor keeps its rules when switched back and forth.
  - Pickers open upward when there is no room below.
  - A private UDP service says it needs a public port, since visitors cannot use UDP services.
- **macOS builds are signed** with burrow's own certificate, so with the macOS firewall on, burrow is asked about once and keeps that answer across `burrow update` (before, every update was asked again, or silently blocked on a Mac with nobody logged in).
- **`burrow edit`:** a stop signal (Ctrl-C, kill) in the first moments, before the editor opens, no longer ends burrow without removing its private copy; it now says nothing was changed and cleans up.
- **Messages and docs:**
  - A DNS lookup that times out is named as DNS.
  - An unset secret variable's hint says the value must match the server's.
  - Generated client allow lists are sorted.
  - The README shows client.yaml, visitor.yaml and the init commands again.

## [0.1.0]

First release.

- Reverse TCP and UDP tunnel. The server config decides what is exposed, and services can be added, changed or removed with a hot reload (file watch or SIGHUP), without restarting the client.
- Noise `NNpsk0` (ChaChaPoly by default, AES-GCM optional) with a separate 256-bit secret per client, secret rotation, constant-work rejection of unknown names, and a client-side allow list that is deny-by-default.
- Adaptive data-channel pool per client, heartbeat, half-close, HTTP and SOCKS5 proxy support for the client. A data channel that goes silent ends after three heartbeats; a client sizes its open connections (answering busy beyond them), UDP peers and waiting data channels from its open file limit, so a misbehaving server cannot exhaust its memory or files; and UDP datagrams larger than 9 KB are forwarded on macOS too. A connection cut off inside the tunnel is reset at both ends, so a truncated transfer never looks complete, and a UDP service follows its target to a new address when DNS changes.
- Optional WebSocket transport (`ws://` behind a TLS proxy, or `wss://` with your own certificate, picked up again from its files when renewed) for servers that are only reachable over HTTP(S). It is off unless configured, and `init server --ws` writes a config without a TCP control port.
- Clients never give up by default: jittered backoff, DNS re-resolved on every attempt, Happy Eyeballs dialing that tries last an address which failed after connecting and first the one that last worked, one outage summary per minute, and optional `give_up_after_secs` / `on_rejected: exit`. Only the server's explicit refusal counts as rejected, so a forwarder in front of a stopped server is retried. Visitors and data channels retry a handshake that a busy server or a proxy dropped, for up to 5 s, and a client whose server no longer knows its session (the name now leads to another server) reconnects.
- **Private services and visitors.** A service without `bind` opens no port on the server; the visitors it lists reach it from their own machine with `burrow visit`, through the server and the client's tunnel. `reach: { CLIENT: any }` lets a visitor pick any target that client allows, like `ssh -L`, and rules such as `CLIENT: [192.168.150.0/25]` narrow it; a host name is checked after the client resolves it. The client's `allow` list still decides, and reload closes the open connections of a visitor whose access was removed.
- **`self`: the server as its own client.** `client: self` exposes something next to the server (a database, a web app, its LAN) without running a client there, and `reach: { self: ... }` lets a visitor reach it; both pass through `self.allow` as a client's `allow` list. The server never dials its own listen, WebSocket or metrics addresses, whatever `self.allow` says, and dials link-local and cloud metadata addresses only when `self.allow` names one on its own.
- **`burrow init visitor` asks the server what the visitor may use** and writes ready forwards; `--update` refreshes them in an existing `visitor.yaml`, keeping your local ports and the rest of the file, and shows the changes first. A SOCKS forward into a client says what that client's `allow` list permits.
- `burrow visit` runs all forwards, one with `visit NAME --local PORT`, or ad hoc with `--service` or `--client --target`; `burrow visitor-config` writes a visitor's config on the server.
- **SOCKS5 for visitors**: `burrow visit --client home --socks 1080`, or `socks: true` on a forward; `visitor-config` writes one for each client in `reach`.
- **Port ranges**: `bind`, `target` and a visitor's `local` take ranges like `30000-30010`; each port reaches the same position in the target range.
- **Several clients per service**: `clients: [a, b]` with `mode: failover` or `balance`. A connection that a client cannot serve, because its target is down, not in its `allow` list or the client is busy, moves to the next client, and that client is skipped for a while (a refusal until it reconnects). `health_check` also checks the target through each client ahead of time.
- **Per-service controls**: `allow_from` and `deny_from` for source addresses (checked again on reload, also for visitors), `proxy_protocol: v1 | v2` to send the real client address to the target, and `bandwidth` to cap each direction.
- **Unix socket targets**: `target: unix:/path`, permitted on the client by `unix:` entries in `allow`.
- **Clients and visitors reload their config on save** (or `SIGHUP`, or `service reload`), like the server: a new `allow`, `server`, `name`, `secret`, `headers`, `ca`, `cipher`, `proxy` or `insecure_plaintext` reconnects the client, other changes apply at once, a visitor opens and closes only the forwards that changed, and open connections keep running, except those whose target a narrowed `allow` no longer permits. A config that does not load, or a name another running client already uses, is rejected and the current one kept, and tried again once the cause is fixed, such as a certificate copied in later. The files are also rechecked every 30 s, so saves through a Docker file mount, into a replaced folder or on a network share are applied too.
- `burrow validate` reports every config problem with file:line:column, a caret, a fix and "did you mean" suggestions, and refuses settings that would have no effect, such as `max_connections` on a UDP service or 0. The same checks run when the server, client or visitor starts and on every reload.
- **`burrow edit`** finds the configs in this folder, of installed services and of running instances, opens one in your editor (`--editor`, `VISUAL` or `EDITOR`), checks it like `validate`, saves it with its owner and permissions only if nobody changed it meanwhile; whatever runs it then applies it by itself.
- **`burrow admin`** opens a web UI for the server in your browser, served on 127.0.0.1 by your own machine and reaching the server through an admin visitor's encrypted channel (`admin: write` or `admin: read` on the visitor, optionally `admin_from`). It shows live clients, services, visitors and traffic, every server setting, and changes server.yaml in place keeping its comments: each change is checked by the server, shown as a diff with its impact (connections it closes, ports it opens) and needs the name typed for destructive ones; the server refuses changes that would lock you out, keeps the last 20 versions for "Revert to before this change", and refuses edits of `listen`, `websocket`, `insecure_plaintext`, `heartbeat.interval_secs`, `limits.handshake_timeout_secs` and `limits.max_handshakes`, which stay in server.yaml. A new client or visitor gets a generated secret, shown once with its ready config and commands. An installed server keeps a config folder it may write when an admin visitor exists (or with `--admin-writable`).
- burrow admin lowers admin access but never grants or widens it: a new admin, `read` to `write`, a new secret on an admin visitor, a wider `admin_from` or a renamed admin visitor is refused, also by a revert, and is made in the file with `burrow edit`.
- **`burrow status`** shows every server running on the machine, however it was started, with no setup and nothing reachable from the network: tables with color in a terminal that fit its width, `-f` for a live screen with rates, `--json` for scripts; a server that does not answer is named, not taken for stopped. Prometheus metrics stay optional (`metrics.listen`, off by default).
- `burrow init`, `client-config`, `gensecret`, `schema` and `completions`. `client-config` takes the client address from `--server` or the server's optional `public_address` and never guesses it from `listen`; a host alone gets its port from `listen`. The client name can be left out when there is only one, `-o FILE` writes a file only you can read, and the format follows the `-o` extension.
- Short forms in config files: `listen: 7835`, `bind: 7840` and `target: 3000` (for `127.0.0.1:3000`), and `server: host` for port 7835.
- `burrow service` installs a server, client or visitor as a system service on Linux (systemd, sandboxed), macOS (launchd) and Windows (SCM), with a dedicated account, a copied config and binary, a port check before install, rotating log files (`--log-file`), `status`, `logs -f`, `list`, `reload`, and `update` with rollback. Commands that restart services finish even when the SSH session that started them runs through the tunnel and drops.
- An installed service always runs from its own copy of the binary (`/usr/local/bin/burrow`, or a root-only folder when other users can change that one), created if missing, so it never depends on where burrow was downloaded, such as a private `/root` folder the service account cannot enter; a service that never started points to `journalctl` for the reason.
- **`burrow update`** downloads the latest release (or `--to VERSION`), verifies it against `SHA256SUMS` and by running it, then replaces this binary and updates the installed services with rollback. `--check` only reports whether a newer release exists, and it points out installed services that still run an older version. It honours `HTTPS_PROXY` and `NO_PROXY`, and a mirror in `BURROW_UPDATE_URL` must use https unless it is on the same machine. Service installs, updates and uninstalls take turns through one lock.
- The same tunnel cannot run twice (an OS file lock), one address cannot take more than its share of the server's unauthenticated handshake slots when they run short, and `-c` can be left out when `server.yaml`, `client.yaml` or `visitor.yaml` is in the current folder.
- Static Linux binaries (amd64, arm64, armv7, armv6, riscv64), macOS (arm64, amd64), Windows (amd64, arm64) and FreeBSD (amd64) builds, and a multi-arch distroless image.
