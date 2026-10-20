# Decisions

Short records. Newest last.

## D1. Transport: raw TCP with a data-channel pool, no HTTP/2 or QUIC as primary

HTTP/2 only lets the client open streams, so the server could not open one per visitor without a control-channel round trip anyway. It also puts every stream behind one TCP connection (head-of-line blocking), has no UDP, and brings a large pre-auth surface (HPACK, SETTINGS, flow control). QUIC is a possible future option if UDP performance matters, but it would add a large dependency and its own TLS. Default: one TCP control channel per client plus one TCP data channel per visitor connection, taken from a pre-established pool.

## D2. Pool per client, not per service

rathole keeps 8 idle channels per TCP service. Because burrow tells the data channel its target in the first frame, any idle channel can serve any service of that client. Default `pool.min_idle: 4`, adaptive up to `pool.max_idle: 64`: when a visitor finds the pool empty, the target idle count doubles; it decays by one each idle minute.

## D3. Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s, one handshake per channel

Every control and data channel runs its own handshake. A full handshake (both sides, one core) costs about 118 us (benches/cipher.rs), which the pool hides from visitors. Details and proof points are in docs/protocol.md.

## D4. Cipher: ChaChaPoly default, AES-GCM opt-in, `ring` backend

Measured on an i5-12600K (AES-NI, VAES, AVX2), seal plus open in one thread (`cargo bench --bench cipher`):

| backend    | cipher      | 16 KiB chunk | 64 KiB chunk |
| ---------- | ----------- | ------------ | ------------ |
| ring       | ChaChaPoly  | 1286 MiB/s   | 1261 MiB/s   |
| ring       | AES-256-GCM | 4591 MiB/s   | 4360 MiB/s   |
| RustCrypto | ChaChaPoly  | 810 MiB/s    | 833 MiB/s    |
| RustCrypto | AES-256-GCM | 864 MiB/s    | 853 MiB/s    |

snow enables `ring` through its `std` feature anyway, so using the `ring-accelerated` resolver adds no new crates. ChaChaPoly stays the default because it is constant-time on every CPU, including ARM boards without AES instructions. A client can set `cipher: aes-gcm` to get about 3.5x on x86 servers. The server accepts both; the choice is bound into the Noise prologue.

## D5. Config parsing: saphyr-parser for YAML and JSON, our own schema checker

serde_yaml is deprecated (0.9.34+deprecated), serde_yml has had soundness and maintenance concerns, and both stop at the first error. saphyr-parser is maintained (pushed 2026-10) and gives byte, line and column markers for every event. JSON is valid YAML 1.2 flow syntax, so the same parser builds the spanned tree for both; `.json` files are first checked with serde_json so JSON-only syntax errors are reported exactly. The structural check is driven by the JSON Schema that schemars derives from the config types, so `burrow schema`, editor validation and `burrow validate` share one definition. After the tree passes, serde (with `deny_unknown_fields`) builds the typed config.

## D6. One validation engine, run everywhere

`burrow server` and `burrow client` run the same checks as `burrow validate` before opening any socket, and the server runs them on every reload. Errors are printed in the same format each time. `burrow validate` is the same engine without starting anything.

## D7. Durations as integer seconds with a `_secs` suffix

No duration-parsing crate, and the unit is obvious from the key.

## D8. Heartbeat interval is server-owned

The server sends its heartbeat interval in the welcome message. The client times out after three missed intervals, so the two sides cannot disagree.

## D9. Insecure plaintext still authenticates

With `insecure_plaintext: true` on both sides, the Noise handshake still runs (so the secret is never exposed and auth stays strong), but data channels copy raw bytes after Connect and Status. The flag is part of the authenticated prologue, and a mismatch is refused after authentication with `Bye` reason plaintext mismatch. Raw bytes carry the end of each direction as a TCP FIN, so when a target closes first the client sends the first FIN on the data channel and holds its TIME_WAIT, unlike encrypted channels, where the server closes first. A client that sustains several hundred such connections per second can run out of local ports; use encrypted mode there, or allow reuse with `net.ipv4.tcp_tw_reuse=1` on Linux.

## D10. Data plane: data-channel pool, not a multiplexed connection

Evidence from code: frp turns yamux on by default (`tcpMux: true`), and chisel multiplexes SSH channels on one WebSocket. Both put every visitor behind one TCP flow-control window. rathole, like burrow, uses one TCP connection per visitor.

Evidence from measurement (docs/benchmark.md): frp is the cleanest experiment, because multiplexing can be switched off in the same code. Without tcpMux, frp's 4-stream throughput rises from 8.8 to 36.0 Gbit/s and its p99 echo latency under a 2 Gbit/s background load drops from 678 to 383 µs (head-of-line blocking gone), but new connections per second fall from 10.0k to 2.9k, because each visitor waits for a fresh TLS connection. A pre-opened pool keeps the first benefit without the second cost: burrow does 33.4 Gbit/s, a p99 of 549 µs under the same load, and 13.5k connections/s.

## D11. Record size: full 64 KiB Noise records for bulk data

Measured with `bench/run.sh` (burrow only, 2 runs each). Single-stream iperf3 through the tunnel: 8 KiB chunks 7.7 Gbit/s, 16 KiB 10.1, 32 KiB 12.2, 65519 B (largest Noise message) 12.8. With 4 streams: 23.0, 28.5, 32.8, 34.7. Small-message latency was unchanged (p50 61 µs in every case) because short reads are sealed from a 512-byte stack buffer. Big buffers exist only while data is flowing: a pump drops them when the socket has no data ready, so idle connections hold no large buffers.

## D12. Allocator: system malloc, except on musl

With mimalloc: throughput unchanged (12.6 vs 12.8 Gbit/s single stream), new connections per second +3% (14.1k vs 13.7k), but RSS with 10k idle connections rose from 167 MB to 273 MB. The cost outweighs the gain, so glibc, macOS, Windows and FreeBSD builds keep the system allocator.

The release Linux binaries are static musl builds, and musl's mallocng fragments under connection churn: in a soak the RSS of a server grew from 6.3 to 16.2 MB in under 2 hours while its live heap stayed at about 1.4 MB. Musl builds therefore use jemalloc (x86_64, aarch64 with 64 KiB pages allowed, riscv64), and mimalloc on 32-bit ARM, where jemalloc does not build with zig. With them the same soak stays flat.

## D13. insecure_plaintext copy path

`tokio::io::copy` uses an 8 KiB buffer: 14 Gbit/s single stream, behind rathole's plain TCP (16). A 64 KiB buffer reached 33 Gbit/s (69 with 4 streams), but a `BufReader` would hold 64 KiB per direction for the life of the connection (about 2.6 GB at 10k idle connections). The raw pump therefore uses the same lazy scheme as the encrypted path. Same throughput, and 10k idle connections cost 172 MB, the same as encrypted mode. The 128 KiB record-reader buffer is also freed once the plaintext stream takes over.

## D14. Listen backlog 4096

std and mio listen with a backlog of 128. A burst of 300 visitors overflowed it (`TcpExtListenOverflows`, "Possible SYN flooding" in dmesg), and some visitors were reset. All listeners are now created with a backlog of 4096, capped by the kernel's `somaxconn`.

## D15. Deferred: pinned-key NK mode and WebSocket transport

NK with a pinned server key plus an inner token would be a second authentication mode next to NNpsk0, which already authenticates both sides without extra key material. Not worth the extra security surface. A WebSocket-over-TLS transport needs rustls, webpki roots and tungstenite on the client, roughly doubling the dependency tree and the pre-auth parsing surface, for networks that only allow HTTPS. HTTP CONNECT and SOCKS5 proxy support already covers many restricted networks. Both stay out until there is a concrete need. The transport is isolated in `handshake.rs` and `src/transport/`, so either can be added later behind a config option. WebSocket was added later, see D17.

## D16. Leak hardening

Long-running behaviour is tested by state, not only by RSS. `tests/heavy.rs` churns TCP connections, UDP peers, client reconnects and reloads, and asserts that sessions, waiting visitors, UDP peers, open file descriptors and live tokio tasks return to their baseline after every round. This found and fixed: control-channel writer tasks that outlived a timed-out session, UDP relay tasks orphaned when their future was cancelled, stale pool waiters, client data channels that survived a client stop, and UDP peers that expired while traffic still flowed in one direction.

## D17. WebSocket transport, opt-in

Some servers can only be reached through an HTTP(S) reverse proxy or a CDN. For those, clients connect over WebSocket (RFC 6455) instead of raw TCP. The feature is off unless the server config has a `websocket` section, and a server configured for WebSocket only opens no TCP control port.

- **WebSocket over HTTP/1.1, not HTTP/2 streams or gRPC.** It passes through every common proxy and through Cloudflare. HTTP/2 extended CONNECT (RFC 8441) is rarely supported by proxies.
- **One WebSocket per data channel, taken from the same pool.** This keeps the head-of-line properties of D10. shellcn multiplexes yamux over a single WebSocket; burrow does not.
- **Noise runs inside the WebSocket.** A TLS-terminating proxy only sees Noise records, and the URL, path and headers grant nothing.
- **No MASQUE (RFC 9298).** CONNECT-UDP needs an HTTP proxy that relays UDP itself, mostly over HTTP/3. burrow already carries each datagram as its own record inside a data channel, so UDP services work over WebSocket unchanged, with the usual UDP-over-TCP latency on packet loss.
- **Keepalive.** Proxies close idle WebSockets (nginx after 60 s by default, Cloudflare after 100 s), and some only count data frames. The server therefore sends an encrypted keepalive record on idle pooled channels every 25 s. A test with an idle-killing proxy fails without it.
- **Crate: tokio-websockets 0.13** (strict RFC compliance, maintained, can use the `ring` backend burrow already ships), plus tokio-rustls, webpki-roots and httparse. The request head is parsed before the upgrade, so wrong paths are refused with 404 and nothing else is exposed.
- **Optional own TLS** (`websocket.tls.cert`/`key`), for servers without a proxy. Clients trust the public roots plus an optional `ca` file.
- **Cost, measured** (docs/benchmark.md, loopback): about 93% of the TCP throughput (11.75 vs 12.62 Gbit/s single stream), 12.0k vs 13.5k new connections per second, and about 1.8x the idle memory at 10k connections (391 vs 220 MiB), from the library's per-connection frame buffers. That is still well below frp (796 MiB) and chisel (719 MiB), which also uses WebSocket.

## D18. Reconnect: never give up by default

Clients retry forever with jittered backoff (250 ms up to 30 s, reset after 10 s of stable connection). DNS is re-resolved on every attempt, so moving the server and updating DNS needs no client change. Addresses are dialed with Happy Eyeballs (RFC 8305: interleaved families, the next attempt 250 ms after the previous one, first success wins), so a stale or black-holed address costs 250 ms instead of the whole connect timeout. An address that accepts TCP but then fails the TLS, WebSocket or Noise step (a load balancer with no backend, an address now used by another host) is tried last from then on, and the address of the last good handshake is tried first, so data channels follow the control channel. During an outage the client logs the first failure and then a summary once a minute. `reconnect.give_up_after_secs` and `reconnect.on_rejected: exit` exist for setups where a supervisor should notice; neither is written by `init`. A rejected client keeps retrying by default, so fixing the server config later brings it back without touching the client machine.

## D19. Default ports 7835-7840

2333 is rathole's default, and 8080, 8081 and 8443 are among the most used ports on any host. burrow uses its own block, none of which is in the services database: 7835 for TCP clients, 7836 for WebSocket behind a proxy (bound to 127.0.0.1), 7837 for WebSocket with its own TLS, 7839 for the local `burrow admin` UI, and 7840 for the example service written by `init`.

## D20. `burrow service` instead of shipped unit files

Hand-written unit files go stale, differ per OS, and leave the hard parts to the user: a service account, file permissions, secrets that `sudo` dropped, log rotation, and updates. `burrow service` does them the same way on every platform, much like `cloudflared service install` and `tailscaled install-system-daemon`.

- **Admin only, system services only.** A per-user service would stop at logout on Linux and macOS. One install path keeps it simple.
- **A dedicated account, created by burrow.** `burrow` (useradd `--system`), `_burrow` (dscl, hidden, id below 500), or a Windows virtual account, which Windows creates with the service. Running as root would give an attacker who gets through the pre-auth parsers the whole machine. The account and the copied binary are removed by the last `uninstall` that used them.
- **Copies, not references.** The config is copied to `/etc/burrow` and the binary to `/usr/local/bin`, because a service must not depend on, or run code from, a folder a normal user can write. The binary is always copied, even from a root-owned folder, so a service never depends on where burrow was downloaded (a 0700 `/root` would make systemd fail with 203/EXEC for the service account) and always runs from one predictable path; there is no official package to defer to, and the unrelated Debian package named `burrow` rules out guessing a package path. When users other than root can write `/usr/local/bin` (Homebrew on Intel Macs), the service gets its own copy in a root-only folder instead.
- **Checks before changes.** The config is validated, every listen port is test-bound and its owner is named, and the instance lock is tried before anything is written. A failed reinstall restarts the previous service.
- **Own log files** with size-based rotation (10 MB, 5 files) rather than journald or the Windows event log, so `burrow service logs` works the same everywhere and a log cannot fill the disk. Startup failures are written there too, so a Windows service that has no console still says why it stopped.
- **Updates:** `burrow service update` replaces the binary only when it is newer, keeps the previous one, restarts every affected service and rolls back if one does not come up. `burrow update` downloads a release itself (D29).
- **Crates:** `windows-service` (Mullvad) for the Windows dispatcher and SCM; systemd and launchd are driven through `systemctl` and `launchctl`, which are their stable interfaces.

## D21. Ask only for what burrow cannot know, but write every field

`listen` says where the server binds on its own machine, usually `0.0.0.0`. Behind NAT, a cloud firewall or DNS it says nothing about how clients reach it, so the client address is never guessed from it. `client-config` takes it from `--server`, or from the optional `public_address` in the server config, so it is typed once for all clients, and otherwise stops with a hint. A host alone gets its port from `listen`, which is the one thing `listen` does tell. `0.0.0.0` is refused; loopback and private addresses are allowed with a note, because tests, LANs and VPNs need them. Notes only warn about something that may not work.

Config files accept short forms (`listen: 7835`, `target: 3000`, `server: host`), but required fields stay required, and `init` still writes `listen`, `bind`, `target` and `allow` in full, because a generated file is also the user's documentation. `init` writes `public_address` only when `--server` is given, because a placeholder would be an invalid value. WebSocket stays off unless its section is written.

## D22. Visitors: private by default, `reach` as an opt-in

Some services should not be public at all: a database, SSH, an admin page. frp (`stcp` with a visitor) and rathole leave this to a second tool or a firewall rule. burrow lets the server relay a visitor to a client without opening a port.

- **The server's config still decides.** A service without `bind` is private, and lists its `visitors`. A visitor reaches only those services, so a leaked visitor secret exposes what was listed and nothing else.
- **`reach: { CLIENT: any }` is the explicit opt-in** for `ssh -L`-style access, where the visitor picks the target. It is bounded by the client's `allow` list, which the client owner controls and which a compromised server cannot widen. `validate` warns about every `reach: any`, because `init client` writes a broad `allow: [127.0.0.0/8]`.
- **Visitors are not clients.** They have their own section, secrets and limits, they never expose anything, and their names are looked up only for visit channels, so a visitor secret cannot open a client session or the other way round. The same constant-work rejection hides which names exist.
- **One channel per local connection**, with the same handshake as data channels. A visitor keeps no long-lived connection, so it has nothing to reconnect, and the server keeps no state for an idle visitor. The cost is one handshake per connection plus one round trip, because the visitor waits for the server's answer before sending data so that a refusal always carries its reason. The server also decrypts and re-encrypts every record between the two channels: on one loopback machine a visit carries 7.8 Gbit/s on one stream and 22.7 Gbit/s on four, against 12.9 and 35.0 for a public service.
- **`local` defaults to `127.0.0.1`,** unlike `bind` on the server: anyone who can reach a forward uses the visitor's access, so sharing it with a network has to be written out, and `validate` warns about `0.0.0.0`.
- **Reload revokes**: open visits re-check their request when the config changes, as a removed client's connections do.
- **TCP only** for now. UDP visitors would need a peer table on the visitor side as well, and can come later.

## D23. Port ranges are one service

A range like `bind: 30000-30010` stays one service with a first port and a count, instead of eleven generated services. `max_connections`, `bandwidth`, metrics and logs then describe what the user wrote. Each public port has its own listener and maps to the same position in the target range; a visit request carries the index. A range holds at most 1000 ports, so a typo cannot open tens of thousands of sockets.

## D24. PROXY protocol written by the server

The server knows the real source address, so it writes the PROXY v1 or v2 header into the tunnel right after Connect, and the client forwards it as data. Clients need no change, the same code covers visits (with the visitor's address), and the header works in `insecure_plaintext` mode too. It is off by default and per service, because a target that does not expect it fails on the first bytes. Checked against nginx `listen ... proxy_protocol` with both versions.

## D25. Several clients, and health checks through the tunnel

A service can list several clients. `failover` takes the first connected one, `balance` rotates new connections; an open connection never moves. Health checks run on the server and use the tunnel itself: a normal Connect to the target through each client, closed at once. This needs no client config, tests the real path including the client's allow list, and works with any target the client can reach. A client is down after `failures` failed checks in a row and up again after one success. When no client passes, connections still go to a connected one, so a broken check cannot take a working service down.

Without health checks, the server still learns from real connections. While it waits for the client's answer it keeps a copy of what the visitor sent (up to 64 KiB, then it stops reading until the answer), so when a client answers that it cannot connect, cannot resolve the target or refuses it, the same connection goes to the next client and the visitor notices nothing. A client that does not answer in time counts as failing too, but its connection moves on only if the visitor has sent nothing yet: the client may still deliver those bytes, and a request must never reach the target twice. A channel that breaks says something about the session, not the target, and counts for nothing; a client that cannot hand over a data channel in time is set aside like one that does not answer, for TCP as for UDP. A refusal comes from the client's config, which only changes when it reconnects, so that client is skipped for the target until then; a target it could not reach is skipped for 10 s, doubling up to 60 s while it keeps failing, and the first success clears it. Skipped clients stay at the end of the list and are still tried when all others fail. A service with one client keeps no copy and works as before. This is what nginx does by default (`proxy_next_upstream`, `max_fails`), and it avoids probing every target through every client every few seconds, which shows up in the target's logs.

## D26. Bandwidth limits

One token bucket per service and direction, shared by all its connections and ports, refilled continuously with a burst of 100 ms (at least 16 KiB). TCP waits for tokens, so senders slow down through normal backpressure; UDP has no backpressure, so datagrams over the rate are dropped, as a router would. Throughput, latency and connection rate without a limit are unchanged (measured against the previous commit).

## D27. Metrics off by default, one snapshot for everything

The metrics endpoint is off unless `metrics.listen` is set, defaults to `127.0.0.1`, and `validate` warns when it is reachable from the network, because it shows client, service and visitor names and traffic. The server builds one status snapshot; `/metrics` renders it as Prometheus text and `burrow status` reads it as JSON from a local endpoint only it can use, so the two can never disagree. The metrics port serves nothing else: the JSON holds more than a scraper needs, such as client addresses. Counters are relaxed atomics updated per read, with no measurable cost.

## D28. A SOCKS5 forward for visitors

`reach` lets a visitor pick any target its client allows, but one forward per target is tedious. A SOCKS5 forward (`socks: true`, or `visit --socks`) reads the target from each SOCKS request and sends it as a normal reach request, so the server and the client's allow list decide exactly as before. Only CONNECT without authentication is supported: the listener is on `127.0.0.1` by default, like any forward.

## D29. Our own updater instead of a crate

`burrow update` downloads a release with the TLS, proxy and HTTP pieces burrow already has, checks it against `SHA256SUMS`, runs it to confirm its version, and hands it to `service update` for rollback. The `self_update` crate would have added 2.3 MB (42%) and about 100 crates to the binary and ignored burrow's proxy settings; the own code adds about 110 KB and one small dependency for gzip.

## D30. WebSocket channels end with TLS close_notify

A data channel over `wss://` used to end by dropping the connection. A TLS proxy in front of the server can treat that as an error and drop the last message of the stream under load, which a test reproduced (80 KB missing out of 8 MB). Channels now shut down the TLS layer below the WebSocket, which sends close_notify and a FIN. A WebSocket Close frame as well would cost 3-5% of new connections per second for no benefit, because the record layer already marks the end of each stream.

## D31. Clients and visitors reload on save, like the server

Everything that changes often lives in the server config, so clients were first left to apply changes on restart. That cut every open connection for what is usually an `allow` edit. Clients and visitors now watch their file (and take `SIGHUP`) through the same reload loop as the server: the new file is checked like `validate`, and a file that does not load is rejected while the running config stays. A client compares field by field: `allow`, `server`, `name`, `secret`, `headers`, `ca`, `cipher`, `proxy` and `insecure_plaintext` make it reconnect, the rest applies to the next connection. `allow` reconnects because the server skips a client that refused a target until that client reconnects (D25); a protocol message could avoid that, at the cost of a wire change right before a release, and the reconnect costs about a second. Connections that are being set up at that very moment fail (measured: a few dozen at 64 parallel connection attempts), since the data channels of the old session close with it. Open connections run on their own data channels and survive the reconnect, except those whose target the new `allow` no longer permits, which are closed as on the server. A new `server` or `name` first takes the instance lock of the new identity, so a reload cannot make two clients run the same tunnel. A visitor has no session: connection fields apply to new connections, and only the forwards that changed are closed or opened.

## D32. A visitor asks the server for its own config

`burrow visitor-config` needs the server config, so a visitor depended on the server's admin for every change. The visit channel now answers one more request, "what may I use?", after the same handshake and secret check as a visit, with the same grants that `visitor-config` writes (one function decides both, so they cannot differ). `burrow init visitor` uses it for a new file, and `--update` merges it into an existing one: forwards you have keep their local ports and names, new grants get free ports, revoked ones are removed, the changes are shown before anything is written, and only the `forwards` block of a YAML file is replaced, so other lines and comments stay. To say what `reach` means, a client sends its `allow` list once per session; the server shows it only to visitors with `reach` for that client, who may already use any target it permits.

## D33. Admin over the visit channel, never a new port

`burrow admin` edits a running server's config from a browser on the admin's machine. The server exposes nothing new: an admin channel is a visit channel with the visitor's secret and a different first request (protocol section 11), so it gets the same anti-enumeration handshake, deadline and limits, and a visitor without rights gets the same `5 forbidden` as any refused visit. Rights are opt-in per visitor in the server config, `admin: read | write`, optionally limited to source addresses with `admin_from`. Only these two levels exist; finer rights can be added later as a new field. A write admin without `admin_from` gets no warning, because that is the simple setup most people run. `validate` warns when an admin visitor also uses services or `reach`, since one leaked secret should not give both. On every reload the session checks its visitor again and ends when the right, the secret or the source is gone, or the level changed. An installed server can write its own config only when it has an admin visitor (or `service install --admin-writable`): then the service gets its own config folder that the service account may write, and otherwise the sandbox stays as tight as before and the UI says honestly that it is read-only. Before each admin write the previous file is kept beside it (`.server.yaml.history/`, the last 20, mode 0600), so a change can be reverted; when that folder cannot be written the history is kept in memory only.

## D34. Schema-driven admin UI with semantic types; completeness by derive

The server describes its config to the UI as a schema of semantic types (`ip_port`, `target`, `cidr_rule`, `ref<client>`, `ref_list<visitor>`, `duration`, `rate`, `secret_ref`, ...) with relations such as "only for TCP services" or "one of client and clients". It never names a widget; the UI decides that a `ref_list<client>` is a multi-select of the clients with their online state. Core screens are laid out by hand and place fields by key, and every field they do not place, including fields a newer server adds, is rendered by a generic renderer from the same types. Types a UI does not know come with a `fallback` chain ending in a text input that the server validates, so an older UI keeps working against a newer server and never shows a broken field. The schema comes from the config model through `#[derive(AdminSchema)]`: every field and every enum variant must carry `#[admin(...)]` or `#[admin(skip = "reason")]`, otherwise the build fails on that field, and the macro checks that the semantic type fits the Rust type. Descriptions and ranges are read from the existing `schemars` and `serde` attributes, so they are written once. Server-delivered UI code was rejected: the browser runs only the UI embedded in the local binary.

## D35. Comment-preserving patcher with verification

An admin change is a list of operations on paths, never text. The server applies them to the raw tree of the file and turns the difference into byte edits on the spans of the existing parser: a changed value replaces its span, a new key is inserted after its siblings with their indentation, a removed key takes its leading comment block with it, and the BOM, line endings and every other byte stay. Each result is parsed again and must give exactly the wanted tree; otherwise nothing is written. The result is then checked by the same function as `validate` and reload, the plan with its diff is shown, and the write uses the same atomic, revision-checked save as `burrow edit`. JSON files have no comments and are re-rendered in key order with their indentation. A second YAML library (`yaml-edit`) was considered and rejected: two parsers could disagree on files that `validate` accepts, and it adds 150-250 KB.

## D36. Local UI security: loopback, fragment token, no cookies, Host/Origin checks

`burrow admin` listens on `127.0.0.1:7839`, or on a free port with a note when 7839 is taken (`--port N` is strict, `--port 0` always picks a free one), and never on another address. Each run serves its pages, routes and `/api` under a fresh 128-bit path prefix (`/r-<hex>/`), so a process that held the port earlier cannot have its poisoned cache entries match this run's URLs, and `/api` is reachable only under the prefix, never from a cached page at a fixed path. Every file is served `Cache-Control: no-store` and the pages add `Clear-Site-Data: "cache"`; remote use goes through `ssh -L`. A 256-bit token is required as the first WebSocket message; it reaches the browser in the URL fragment of a 0600 launch file in the private instance folder, so it is never on a command line, in a request or a Referer, and the page removes it from the address bar. There are no cookies, which are not isolated by port, and no CORS headers. Every request must name the local host and port in `Host` (against DNS rebinding), the WebSocket upgrade must come from exactly that `Origin`, and `Sec-Fetch-Site`, when sent, must be `same-origin` or `none`. Pages carry a strict CSP without inline scripts, and server-provided text is only ever inserted as text. The bridge in `burrow admin` passes admin messages through without reading them, so it does not have to change when the protocol grows.

## D37. Leptos CSR, own design system, built once and embedded

The UI is Rust compiled to WebAssembly with Leptos in client-side rendering mode, so it shares the value parsers with the server and the product stays one language. Leptos has fine-grained signals, which suit large live forms and once-a-second status, and it is the most used and documented Rust web framework; Yew and Dioxus are larger or slower to update, Sycamore has few users. The component kit is our own small set of controls with plain CSS and design tokens: the main Leptos kit is pinned to an older Leptos and its look would have to be fought. CI builds the WebAssembly once, compresses it and embeds the same bytes into every release target, with a size budget; a plain `cargo build` without the wasm toolchain embeds a placeholder page instead.

## D38. `self`: the server as its own client, behind two gates

A server often runs next to things worth exposing (a database, a web app, its own LAN), and needing a client process on the same machine for them was pointless. `self` is one reserved name: the client of a service, a key of `reach`, and a config section, and a real client named `self` is refused. It is implemented as a client that runs inside the server: a connection for `self` gets an in-memory channel whose other end is served by the same target code as a remote client (DNS, allow check after resolving, reach rules, Unix sockets, UDP peers, closing what a reload no longer permits). Routing, failover, health checks, limits, the PROXY header and the relay do not know the difference, because they see a channel like any other. The channel carries the usual records unencrypted (a zero tag, same framing, so an end of stream still differs from an abort); only the in-memory pair can be built that way, so no network transport and no config flag can reach it.

- **Two gates.** A visitor's `reach: self` rules are checked first, then `self.allow`, so the effective set is their intersection, as `reach` and a client's `allow` list are for remote clients. Without `self.allow`, `self` services reach their own target and `reach: self` is a validate error. `self.allow` covering a whole address family is warned about, like `allow_all` on a client.
- **What the server never dials for itself:** its own listen, WebSocket and metrics addresses (any address on such a port when it listens on all of them), and link-local or cloud metadata addresses (169.254.0.0/16, fe80::/10, the AWS IPv6 endpoint fd00:ec2::254 and Alibaba's 100.100.100.200) unless `self.allow` names the address on its own; a range covering them does not count. One function decides this for `validate`, the admin UI and the data path.
- **Status:** `self` is listed as a client marked `local`, always connected, whenever the config uses it; remote clients keep their exact JSON.
- **Cost, measured on loopback** (iperf3 and loadgen): 22 Gbit/s on one stream, 71 Gbit/s on four, 16-18k new connections per second, against 13 and 36 Gbit/s and 14k for a remote client on the same machine, which was unchanged by the seam.
