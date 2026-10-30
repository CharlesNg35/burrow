# Research notes

What burrow learned from other reverse tunnels before it was built: what each does well and badly, and what burrow took from it. The resulting choices are recorded in [decisions.md](decisions.md).

Sources read (shallow clones in /tmp/refs, ideas only, no code copied): rathole, frp, bore, chisel, rtun (snsinfu/reverse-tunnel).

## rathole (Rust)

- Good: control channel plus separate data channels; the server keeps a pre-established pool (8 per TCP service, 2 per UDP) and asks for a replacement each time a visitor takes one, so the first byte rarely waits for a handshake. Noise via `snowstorm` (any pattern, default `Noise_NK`). Hot reload diffs services and restarts only the changed ones. Application heartbeat with a client-side timeout. TCP keepalive and nodelay are configurable.
- Weak: the pool is per service, so 50 services cost 400 idle connections. The data-channel type (TCP/UDP) is sent per channel, but the target lives in the client config, so the client must restart to add a service. Auth is a SHA-256 of `token || nonce`, layered on top of a transport that may already be authenticated. Config errors come from `toml` (first error only, no suggestions). UDP sends every datagram over a single data channel, with a `bincode` header length read from the wire.
- Adopt: the control/data split, an eagerly refilled pool, a per-channel first command, and a hot-reload diff.
- Avoid: per-service pools, a separate nonce/digest auth layer, and frames sized from peer bytes before auth.

## frp (Go)

- Good: very complete. `poolCount` pre-opens work connections. Proxies can be added through an admin API.
- Weak: yamux multiplexing is on by default (`tcpMux: true`), so every stream shares one TCP connection and suffers head-of-line blocking under loss. The client defines the proxies. A shared token with no forward secrecy unless TLS is turned on.
- Adopt: the idea of a client-level (not service-level) pool.
- Avoid: multiplexing as the default.

## bore (Rust)

- Good: tiny. Null-delimited JSON frames with a hard `MAX_FRAME_LENGTH` (256) and a network timeout on every pre-auth read. HMAC challenge/response. Each visitor is parked in a map keyed by UUID until the client opens a matching connection, with a 10 s expiry.
- Weak: no encryption, no pool (every visitor pays a TCP round trip plus auth), TCP only.
- Adopt: strict size caps and timeouts on everything that happens before auth.

## chisel (Go)

- Good: SSH over WebSocket goes through HTTP proxies. Uses jpillora/backoff with a max retry interval and resets after a successful connection.
- Weak: SSH channels multiplexed on one connection (head-of-line blocking). A large dependency surface (SSH plus HTTP plus WebSocket) runs before authentication.
- Adopt: backoff reset after a stable connection. WebSocket only as an optional fallback, with our own crypto inside it.

## rtun (Go)

- Good: the server config owns which ports each agent key may bind. Per-session UDP idle timeout (300 s).
- Weak: one WebSocket per TCP session, bearer key in a header (plaintext unless TLS fronts it).
- Adopt: per-peer UDP sessions with an idle timeout; the server is authoritative over exposure.

## What burrow takes

1. The server is authoritative. The client receives the service list over the control channel, so adding a service needs no client change.
2. One adaptive pool **per client** (not per service). The data channel learns its target from its first frame.
3. Noise `NNpsk0` per channel: the PSK authenticates and the ephemeral keys give forward secrecy. No extra auth layer.
4. Fixed-size pre-auth reads (bore-style caps), a semaphore on handshakes, timeouts everywhere.
5. Validation that reports every config problem with spans and suggestions; none of the five does this.
6. Authenticated EOF frames, so half-close and truncation are explicit.
