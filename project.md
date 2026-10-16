Build "burrow": a fast, secure, lightweight reverse tunnel (NAT traversal), in Rust. Work end to end without stopping to ask questions. Make decisions, record them briefly in docs/decisions.md, and keep going.

## Goal

A client behind NAT connects out to a public server. The SERVER's config decides what is exposed: each service maps a public bind address on the server to a target address on the client side (for example 0.0.0.0:4000 -> 127.0.0.1:3000, or 2222 -> 192.168.1.50:22). The client defines no services. Adding or removing a service is a server config edit, with no client change and no restart.

Not in scope: web UI, SOCKS, HTTP routing, ACME, SSH. Keep it small.

## Language and stack

Rust (stable, pinned in rust-toolchain.toml), tokio, tracing, serde, clap. #![forbid(unsafe_code)] unless a measured performance gain justifies an exception. Verify every crate's maintenance status on crates.io before using it (for example, check whether a YAML crate is archived). Keep dependencies minimal.

## Step 0: research (before writing code)

Clone these into /tmp/refs (outside the repo) and read the relevant code: rathole-org/rathole, fatedier/frp, ekzhang/bore, jpillora/chisel, snsinfu/reverse-tunnel. Focus on: control channel and data channel design, data-channel pooling, heartbeat, reconnect and backoff, config hot reload, buffer and copy strategy, TCP_NODELAY and keepalive handling, UDP forwarding, Noise usage in rathole, and how each reports config errors.
Write docs/research.md (one page max): what each does well, what you will adopt, what you will avoid. Take ideas, not code. Do not copy code verbatim. If anything is adapted, keep the original license attribution.

## Security design

Default: Noise with a pre-shared key, pattern Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s (snow crate; confirm the exact pattern string and psk placement against the Noise spec and the crate, and document in docs/protocol.md).
- Each client has its own secret. The secret is the Noise PSK. There is no default or shared secret. The client config needs only its server address, name, secret and allow list: no server public key, no certificates, no domain.
- Mutual authentication comes from the PSK. Forward secrecy comes from the ephemeral X25519 exchange. The client sends an encrypted hello as its first transport message and the server drops the connection if it fails to decrypt. Verify the client authenticates the server through the second handshake message, and document exactly where each side is proven.
- Secrets must be 256-bit random. Reject anything weaker at config load and in "burrow validate". An attacker who captures a handshake can test guesses offline, so a strong random secret is a hard requirement. Document this clearly in the README.
- The server needs to know which PSK to use at the first message. The client sends its non-secret name in clear before the handshake. An unknown name must fail exactly like a wrong secret (same response, same timing: run the handshake against a dummy secret) so names cannot be enumerated.
- Secret rotation: a client may have several valid secrets at once (list), so you can roll without downtime.
- Optional stretch mode: pinned server key (Noise_NK_25519_ChaChaPoly_BLAKE2s) plus a token sent inside the encrypted channel. Keep it only if it stays simple.
- Plaintext only with an explicit "insecure_plaintext: true" and a loud startup warning.
- Client-side allow list, default deny: the client must list allowed target CIDRs (optionally with ports), or set an explicit allow-all. A compromised server must not reach anything outside it. Enforce after DNS resolution, at connect time.
- Hardening: handshake and auth timeouts, a cap on concurrent unauthenticated handshakes, max frame size, max pending connections, per-service and per-client connection limits, no secrets in logs or error output, warn if the config file is world-readable, graceful shutdown on SIGTERM, no allocation sized by peer input before authentication.
- Transport decision to record in docs/decisions.md: do NOT use HTTP/2 or QUIC as the primary transport. Reasons: HTTP/2 only lets the client open streams, adds TCP head-of-line blocking on a shared connection, has no UDP, and has a larger pre-auth attack surface. QUIC is a possible future option if UDP performance matters. The default is raw TCP with a data-channel pool.
- Optional transport, only after the core is done and benchmarked: WebSocket over TLS for networks that only allow HTTPS, with the Noise session running INSIDE the WebSocket so a TLS-terminating proxy still cannot read the traffic. Keep it behind a config option.

## Design requirements

- Data plane: decide between rathole-style (control channel plus on-demand data channels with a pre-established, adaptive pool) and a single multiplexed connection. Justify the choice in docs/decisions.md with evidence from the code you read and a micro-benchmark. Default expectation: data-channel pool, to avoid head-of-line blocking and cut connection setup latency.
- TCP and UDP services. Application-layer heartbeat on the control channel with timeouts on both sides, automatic client reconnect with exponential backoff and jitter, TCP_NODELAY default on, TCP keepalive configurable. Half-close handled correctly.
- Server hot reload: file watch and SIGHUP. Add, remove, or change services without dropping the client connection or other services. Invalid config on reload is rejected, the old one keeps running, and the error is printed in the same format as "burrow validate".
- Optional rathole feature to include: client connecting through an HTTP or SOCKS5 proxy. Include only what stays simple.
- Wire format specification goes in docs/protocol.md, not in code comments.

## Configuration

YAML (preferred) and JSON, same schema, format detected from the file extension. ${ENV_VAR} expansion for secrets. Unknown fields are errors (deny_unknown_fields), never silently ignored. Illustrative schema, refine as needed:

server.yaml
  listen: 0.0.0.0:2333
  clients:
    home:
      secret: ${HOME_SECRET}
  services:
    web:
      client: home
      bind: 0.0.0.0:4000
      target: 127.0.0.1:3000
    dns:
      client: home
      protocol: udp
      bind: 0.0.0.0:5353
      target: 192.168.1.1:53

client.yaml
  server: myserver.com:2333
  name: home
  secret: ${HOME_SECRET}
  allow:
    - 127.0.0.0/8
    - 192.168.1.0/24

## CLI and UX (a first-class requirement)

Commands:
- burrow server -c server.yaml
- burrow client -c client.yaml
- burrow validate -c file [--role server|client] [--strict] [--format text|json]
- burrow init server|client [--format yaml|json] [-o path] [--force] [--server host:port] [--name NAME] [--client-name NAME]
- burrow client-config NAME -c server.yaml --server host:port [--format yaml|json]   prints a ready client config for a client defined in the server config
- burrow gensecret
- burrow schema server|client   prints the JSON Schema, for editor autocomplete and validation
- burrow completions bash|zsh|fish
- burrow version

init:
- "burrow init server" writes a working server config with a fresh 256-bit secret for a first client and one example service, then prints the exact client config to copy (or the client-config command to run). Never overwrite an existing file without --force.
- "burrow init client" writes a client config. With flags it is fully non-interactive. On a TTY with missing flags, ask only for what is missing (server address, name, secret). No prompts when stdin is not a TTY.
- Generated files contain no comments. Keep them minimal and valid, with the secret as a literal (or ${ENV} with --env-secret). Set file mode 0600.

validate (and config errors at startup and on reload):
- Report ALL problems in one run, not just the first, sorted by position. Exit code 0 valid, 1 invalid, 2 usage error. Warnings do not fail unless --strict.
- Role is auto-detected from the content and overridable with --role.
- Every error has: file:line:column, the offending source line with a caret under it, what is wrong in plain words, and a concrete fix. "Did you mean" suggestions for unknown fields, unknown client names, and unknown enum values (edit distance). List the valid options when the set is small.
- Cross-checks: service references an undefined client; duplicate bind addresses; bind equals the control port; invalid address or port; invalid CIDR in allow; unknown protocol; secret missing, too short, or not decodable; unset environment variable (name the variable and show how to set it); empty services list (warning); client allow list missing or allow-all (warning); world-readable config file (warning); insecure_plaintext enabled (warning); a sensitive port such as 22 or 3306 bound on 0.0.0.0 (warning, suggest 127.0.0.1).
- Never print secret values in messages, only their length or a short fingerprint.
- Colors only on a TTY, respect NO_COLOR, and --format json for editors and CI.

Target message style (illustrative):

  error: unknown client "hom" referenced by service "web"
    --> server.yaml:12:15
     |
  12 |       client: hom
     |               ^^^ no client with this name
     = help: did you mean "home"? defined clients: home, office

  error: secret for client "home" is too short (12 bytes, need 32)
    --> server.yaml:5:15
     = help: generate one with: burrow gensecret

  error: unknown field "bnd" in service "web"
    --> server.yaml:11:7
     = help: did you mean "bind"? valid fields: client, bind, target, protocol

Runtime errors get the same care: no wall of text, one line saying what happened and one hint. Examples: port already in use (suggest another port and "ss -ltnp"), handshake rejected (say "wrong secret or unknown client name", never reveal which), server unreachable (show the address tried and that retries continue with backoff), target denied by the allow list (show the target and the allow entry to add). Logging with tracing, info by default, --log-level and a JSON log option, --quiet.

## Performance

Performance is a top requirement. Release profile: lto fat, codegen-units 1, panic abort, strip. Avoid per-connection allocations in the hot path, use zero-copy or efficient bidirectional copying, tune buffer sizes and Noise chunk sizes by measurement, and consider mimalloc only if it measurably helps. Compare ChaCha20-Poly1305 and AES-GCM on the build machine and record the result in docs/decisions.md.
Build a reproducible benchmark harness in bench/ (scripts, no hand-edited results) comparing burrow against rathole and frp (and chisel if easy) on: throughput (iperf3), latency (small request/response), new connections per second, and memory with 1k and 10k idle connections. Put results in docs/benchmark.md with the exact commands and machine info. Goal: match or beat rathole. If you don't, say so and explain why.

## Code quality

Clean, small modules with clear names (config, validate, diagnostics, init, protocol, crypto/noise, transport, server, client, control, pool, forward tcp/udp, allowlist, cli). Idiomatic Rust, clippy clean with -D warnings, rustfmt.
NO comments and NO doc comments unless strictly necessary (a non-obvious invariant or a safety argument, in one line). Names and structure carry the meaning. Protocol and design explanations live in docs/, not in code. README stays short: quickstart (init, run, validate), config reference, security notes, Docker usage, benchmark summary.

## Testing (heavy, and actually run it)

- Unit tests: config parsing and validation, weak-secret rejection, allow list, protocol framing, handshake, auth failures.
- Diagnostics tests: a table of broken configs (typos, wrong types, missing fields, bad CIDR, unknown client, duplicate binds, unset env var, weak secret) with snapshot tests of the exact messages, line and column numbers, and suggestions; YAML and JSON both; multiple errors in one file all reported; no secret ever appears in output.
- init tests: generated server and client configs pass validate, the printed client config connects to the generated server, existing files are not overwritten without --force, file mode is 0600.
- Security tests: wrong secret, unknown client name (same response and timing as a wrong secret), replayed handshake, truncated or modified handshake messages, secret rotation with two valid secrets, connection limits under a flood of unauthenticated handshakes.
- Integration tests (in-process server and client, real sockets): forwarding correctness, large transfer with checksum, many parallel connections, half-close, client reconnect after server restart, server hot reload add/remove/change (including a rejected invalid reload), allow-list denial, UDP forwarding, graceful shutdown.
- Fuzz the handshake, frame parser, and config loader (cargo-fuzz), with a short run in CI and the corpus committed.
- Heavy tests behind an opt-in flag or a manual workflow: 10k concurrent connections, a sustained throughput soak (30+ minutes), chaos (dropped connections, killed processes, tc netem or toxiproxy if available), and a memory-leak check over the soak.
- Run everything you write and fix what fails. Never claim a test passed unless you ran it. List anything you could not run, and why.

## CI/CD (GitHub Actions)

- ci.yml on push and PR: fmt, clippy -D warnings, tests, cargo audit and cargo deny, short fuzz, build.
- release.yml on tags v*.*.*, plus workflow_dispatch for a dry run that builds without pushing:
  - static binaries (musl) for linux amd64 and arm64, plus macOS and windows if they build cleanly; cross-compile instead of QEMU for speed.
  - GitHub Release with the binaries, SHA256SUMS, and notes from CHANGELOG.md.
  - multi-arch Docker image (linux/amd64, linux/arm64) pushed to ghcr.io/<owner>/burrow using GITHUB_TOKEN, with tags: full version, major.minor, latest. Optional Docker Hub push when DOCKERHUB_USERNAME and DOCKERHUB_TOKEN secrets exist. Build provenance attestation and SBOM.
  - least-privilege permissions, caching, pinned action versions.
- Dockerfile: static binary on distroless static or scratch, non-root user, config mounted at /etc/burrow, small image. Include docker-compose.example.yml for server and client, and a systemd unit example. A "burrow validate" step in the image docs so users can check config before starting.

## Process and deliverables

1. Research (docs/research.md).
2. Design decisions (docs/decisions.md), then implement in phases: config and diagnostics and validate and init first (so every later phase gets good errors for free), TCP with Noise PSK auth, pool and heartbeat and reconnect, hot reload, allow list and limits, UDP, optional features, Docker, CI/CD, benchmarks, heavy tests, then the optional WebSocket transport.
3. Commit in small, meaningful commits with clear messages.
4. Do NOT push, tag, or publish anything. Prepare the workflows only. Do not commit secrets.
5. Final report: what was built, test results (including heavy ones), benchmark table, known limits, and exactly which repo secrets I must set and the commands to tag a first release.
