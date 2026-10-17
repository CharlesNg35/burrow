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

With `insecure_plaintext: true` on both sides, the Noise handshake still runs (so the secret is never exposed and auth stays strong), but data channels copy raw bytes after the hello. The mode is bound into the prologue, so a mismatch fails the handshake.

## D10. Data plane: data-channel pool, not a multiplexed connection

Evidence from code: frp turns yamux on by default (`tcpMux: true`), and chisel multiplexes SSH channels on one WebSocket. Both put every visitor behind one TCP flow-control window. rathole, like burrow, uses one TCP connection per visitor.

Evidence from measurement (docs/benchmark.md): frp is the cleanest experiment, because multiplexing can be switched off in the same code. Without tcpMux, frp's 4-stream throughput rises from 9.3 to 38.3 Gbit/s and its p99 echo latency under a 2 Gbit/s background load drops from 694 to 425 µs (head-of-line blocking gone), but new connections per second fall from 10.4k to 2.9k, because each visitor waits for a fresh TLS connection. A pre-opened pool keeps the first benefit without the second cost: burrow does 35.1 Gbit/s, a p99 of 498 µs under the same load, and 13.8k connections/s.

## D11. Record size: full 64 KiB Noise records for bulk data

Measured with `bench/run.sh` (burrow only, 2 runs each). Single-stream iperf3 through the tunnel: 8 KiB chunks 7.7 Gbit/s, 16 KiB 10.1, 32 KiB 12.2, 65519 B (largest Noise message) 12.8. With 4 streams: 23.0, 28.5, 32.8, 34.7. Small-message latency was unchanged (p50 61 µs in every case) because short reads are sealed from a 512-byte stack buffer. Big buffers exist only while data is flowing: a pump drops them when the socket has no data ready, so idle connections hold no large buffers.

## D12. Allocator: system malloc, not mimalloc

With mimalloc: throughput unchanged (12.6 vs 12.8 Gbit/s single stream), new connections per second +3% (14.1k vs 13.7k), but RSS with 10k idle connections rose from 167 MB to 273 MB. The cost outweighs the gain, so the dependency was removed.

## D13. insecure_plaintext copy path

`tokio::io::copy` uses an 8 KiB buffer: 14 Gbit/s single stream, behind rathole's plain TCP (16). A 64 KiB buffer reached 33 Gbit/s (69 with 4 streams), but a `BufReader` would hold 64 KiB per direction for the life of the connection (about 2.6 GB at 10k idle connections). The raw pump therefore uses the same lazy scheme as the encrypted path. Same throughput, and 10k idle connections cost 172 MB, the same as encrypted mode. The 128 KiB record-reader buffer is also freed once the plaintext stream takes over.

## D14. Listen backlog 4096

std and mio listen with a backlog of 128. A burst of 300 visitors overflowed it (`TcpExtListenOverflows`, "Possible SYN flooding" in dmesg), and some visitors were reset. All listeners are now created with a backlog of 4096, capped by the kernel's `somaxconn`.

## D15. Deferred: pinned-key NK mode and WebSocket transport

NK with a pinned server key plus an inner token would be a second authentication mode next to NNpsk0, which already authenticates both sides without extra key material. Not worth the extra security surface. A WebSocket-over-TLS transport needs rustls, webpki roots and tungstenite on the client, roughly doubling the dependency tree and the pre-auth parsing surface, for networks that only allow HTTPS. HTTP CONNECT and SOCKS5 proxy support already covers many restricted networks. Both stay out until there is a concrete need. The transport is isolated in `handshake.rs` and `transport.rs`, so either can be added later behind a config option.

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
- **Cost, measured** (docs/benchmark.md, loopback): about 93% of the TCP throughput (11.9 vs 12.7 Gbit/s single stream), 12.3k vs 13.8k new connections per second, and 2x the idle memory at 10k connections (348 vs 176 MiB), from the library's per-connection frame buffers. That is still well below frp (784 MiB) and chisel (708 MiB), which also uses WebSocket.

## D18. Reconnect: never give up by default

Clients retry forever with jittered backoff (250 ms up to 30 s, reset after 10 s of stable connection). DNS is re-resolved on every attempt, so moving the server and updating DNS needs no client change. Addresses are dialed with Happy Eyeballs (RFC 8305: interleaved families, the next attempt 250 ms after the previous one, first success wins), so a stale or black-holed address costs 250 ms instead of the whole connect timeout. During an outage the client logs the first failure and then a summary once a minute. `reconnect.give_up_after_secs` and `reconnect.on_rejected: exit` exist for setups where a supervisor should notice; neither is written by `init`. A rejected client keeps retrying by default, so fixing the server config later brings it back without touching the client machine.

## D19. Default ports 7835-7840

2333 is rathole's default, and 8080, 8081 and 8443 are among the most used ports on any host. burrow uses its own block, none of which is in the services database: 7835 for TCP clients, 7836 for WebSocket behind a proxy (bound to 127.0.0.1), 7837 for WebSocket with its own TLS, and 7840 for the example service written by `init`.

## D20. `burrow service` instead of shipped unit files

Hand-written unit files go stale, differ per OS, and leave the hard parts to the user: a service account, file permissions, secrets that `sudo` dropped, log rotation, and updates. `burrow service` does them the same way on every platform, much like `cloudflared service install` and `tailscaled install-system-daemon`.

- **Admin only, system services only.** A per-user service would stop at logout on Linux and macOS. One install path keeps it simple.
- **A dedicated account, created by burrow.** `burrow` (useradd `--system`), `_burrow` (dscl, hidden, id below 500), or a Windows virtual account, which Windows creates with the service. Running as root would give an attacker who gets through the pre-auth parsers the whole machine. The account and the copied binary are removed by the last `uninstall` that used them.
- **Copies, not references.** The config is copied to `/etc/burrow` and the binary to `/usr/local/bin`, because a service must not depend on, or run code from, a folder a normal user can write. A binary in a root-owned, non-writable folder (a package manager) is used where it is.
- **Checks before changes.** The config is validated, every listen port is test-bound and its owner is named, and the instance lock is tried before anything is written. A failed reinstall restarts the previous service.
- **Own log files** with size-based rotation (10 MB, 5 files) rather than journald or the Windows event log, so `burrow service logs` works the same everywhere and a log cannot fill the disk. Startup failures are written there too, so a Windows service that has no console still says why it stopped.
- **Updates:** `burrow service update` replaces the binary only when it is newer, keeps the previous one, restarts every affected service and rolls back if one does not come up. A self-download from GitHub releases is left for later.
- **Crates:** `windows-service` (Mullvad) for the Windows dispatcher and SCM; systemd and launchd are driven through `systemctl` and `launchctl`, which are their stable interfaces.

## D21. Ask only for what burrow cannot know, but write every field

`listen` says where the server binds on its own machine, usually `0.0.0.0`. Behind NAT, a cloud firewall or DNS it says nothing about how clients reach it, so the client address is never guessed from it. `client-config` takes it from `--server`, or from the optional `public_address` in the server config, so it is typed once for all clients, and otherwise stops with a hint. A host alone gets its port from `listen`, which is the one thing `listen` does tell. `0.0.0.0` is refused; loopback and private addresses are allowed with a note, because tests, LANs and VPNs need them. Notes only warn about something that may not work.

Config files accept short forms (`listen: 7835`, `target: 3000`, `server: host`), but required fields stay required, and `init` still writes `listen`, `bind`, `target` and `allow` in full, because a generated file is also the user's documentation. `init` writes `public_address` only when `--server` is given, because a placeholder would be an invalid value. WebSocket stays off unless its section is written.

## D22. Visitors: private by default, `reach` as an opt-in

Some services should not be public at all: a database, SSH, an admin page. frp (`stcp` with a visitor) and rathole leave this to a second tool or a firewall rule. burrow lets the server relay a visitor to a client without opening a port.

- **The server's config still decides.** A service without `bind` is private, and lists its `visitors`. A visitor reaches only those services, so a leaked visitor secret exposes what was listed and nothing else.
- **`reach: { CLIENT: any }` is the explicit opt-in** for `ssh -L`-style access, where the visitor picks the target. It is bounded by the client's `allow` list, which the client owner controls and which a compromised server cannot widen. `validate` warns about every `reach`, because `init client` writes a broad `allow: [127.0.0.0/8]`.
- **Visitors are not clients.** They have their own section, secrets and limits, they never expose anything, and their names are looked up only for visit channels, so a visitor secret cannot open a client session or the other way round. The same constant-work rejection hides which names exist.
- **One channel per local connection**, with the same handshake as data channels. A visitor keeps no long-lived connection, so it has nothing to reconnect, and the server keeps no state for an idle visitor. The cost is one handshake per connection plus one round trip, because the visitor waits for the server's answer before sending data so that a refusal always carries its reason. The server also decrypts and re-encrypts every record between the two channels: on one loopback machine a visit carries 7.8 Gbit/s on one stream and 22.7 Gbit/s on four, against 12.9 and 35.0 for a public service.
- **`local` defaults to `127.0.0.1`,** unlike `bind` on the server: anyone who can reach a forward uses the visitor's access, so sharing it with a network has to be written out, and `validate` warns about `0.0.0.0`.
- **Reload revokes**: open visits re-check their request when the config changes, as a removed client's connections do.
- **TCP only** for now. UDP visitors would need a peer table on the visitor side as well, and can come later.
