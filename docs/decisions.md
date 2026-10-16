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
