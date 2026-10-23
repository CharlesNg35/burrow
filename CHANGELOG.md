# Changelog

## [0.1.2]

Limits you can set and see.

- **`max_connections` in client.yaml** (default 16384, 1 to 1000000): the most connections open through a client at once. A saved change applies at once without a reconnect, and open connections keep running when it is lowered. If the open file limit is too small for it, the client lowers it, with one warning at start and after each change when you set the value yourself.
- **UDP peer limits:** `max_udp_peers` in client.yaml (default 8192) and `udp.max_peers` in server.yaml (default 4096 per service port, editable in the admin UI), 1 to 1000000, applied at once like `max_connections`; the client's `max_udp_peers` is also lowered to fit the open file limit. A client now follows the server's per-port limit instead of its own fixed 4096.
- **Seeing a limit:** a client at its `max_connections` logs at most one warning a minute with the number refused since the last one, the server's log points to the client's `max_connections`, `burrow status` and the admin UI mark the client "at limit", the service's REFUSED count includes those refusals, and `burrow_client_busy_total` counts them per client.
- **UDP peers are kept 2 minutes** without traffic by default (`udp.session_timeout_secs`, was 60 s) on the server, the client and the visitor, the least RFC 4787 asks of a NAT, so quiet UDP flows such as games and VPNs keep their mapping.
- **Running at scale** in docs/monitoring.md: memory per connection, the limits to raise, and the Linux limits that run out first.
- `burrow validate` and burrow admin tell a UDP service with no `bind` to add a public port, instead of suggesting visitors.
- The `burrow admin` note for a moved port says "taken" only when the port is in use, gives the real error otherwise, and says "open the new link" only when a link is shown.
- `sudo burrow admin` keeps its admin session in root's own home instead of the user's, so later runs without sudo keep their session, and sudo runs still reconnect their open tab.
- An open admin tab keeps retrying every 2 s for 5 minutes after `burrow admin` stops (was 30 s), so a restarted `burrow admin` reconnects it instead of opening a second tab.
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
