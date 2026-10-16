# burrow wire protocol, version 1

All integers are big-endian. "Record" means one Noise transport message with a 2-byte length prefix.

## 1. Channels

A client opens TCP connections to the server's `listen` address. Each connection is either:

- a **control channel**: one per client, long-lived, carries commands and heartbeats;
- a **data channel**: one per forwarded TCP connection, or one per UDP service; opened ahead of time into the server's pool.

Every channel, control or data, does its own full Noise handshake.

## 2. Carriers

The byte stream below can be carried in two ways. Both carry exactly the same bytes.

- **TCP** (`listen`): the stream is the TCP connection.
- **WebSocket** (`websocket`, opt-in): the client sends an HTTP/1.1 `GET <path>` upgrade request (RFC 6455), optionally over TLS (`wss://`), with any extra `headers` from its config. Before upgrading, the server reads at most 8 KiB of request head and answers `404` for any other path, `426` for a non-upgrade request on the path, and `400` or `431` for malformed or oversized heads. After the upgrade, the stream is the concatenation of the payloads of binary messages, in order. The sender batches records into messages of up to 128 KiB, and messages over 1 MiB are rejected. Text, ping and pong messages carry no stream data, and a close message ends the stream. Authentication is still the Noise handshake below: the URL, path and headers are not secrets.

On a WebSocket carrier, data channels always use encrypted records, even with `insecure_plaintext`, because WebSocket has no half-close for the raw-stream mode in section 8.

The WebSocket upgrade and the Noise handshake share one `limits.handshake_timeout_secs` deadline and one `limits.max_handshakes` slot.

## 3. Preamble (client → server, clear text)

| field    | size | value                                                                             |
| -------- | ---- | --------------------------------------------------------------------------------- |
| magic    | 4    | `BRW1`                                                                            |
| kind     | 1    | `1` control, `2` data                                                             |
| flags    | 1    | bit 0: AES-GCM (else ChaChaPoly); bit 1: insecure_plaintext; other bits must be 0 |
| name_len | 1    | 1..=64                                                                            |
| name     | n    | client name, `[A-Za-z0-9._-]`                                                     |
| msg1     | 48   | Noise message 1                                                                   |

The server reads exactly 7 bytes, validates them, then reads exactly `name_len + 48` bytes into a fixed stack buffer. Nothing the peer sends before authentication controls an allocation size.

## 4. Noise handshake

- Pattern: `Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s`, or `Noise_NNpsk0_25519_AESGCM_BLAKE2s` when flag bit 0 is set (snow 0.10, `Builder::psk(0, key)`).
- Message tokens (Noise spec section 9.4): `-> psk, e` then `<- e, ee`.
- PSK: the client's 32-byte secret (`clients.<name>.secret`). The spec requires 256 bits of entropy; burrow rejects anything that does not decode to exactly 32 bytes, or that has fewer than 16 distinct byte values.
- Prologue: `"burrow/1\0" || preamble-without-msg1`. Kind, flags and name are therefore authenticated: changing any of them makes message 1 fail to decrypt.
- msg1 = 32-byte ephemeral `e` + 16-byte tag of the empty payload. Because the `psk` token runs first, the payload is encrypted with a key derived from the PSK (spec section 9.2: in PSK handshakes `e` is followed by `MixKey(e)`).
- msg2 = 32-byte ephemeral `e` + 16-byte tag (empty payload), sent server → client.

### Server-side secret lookup (anti-enumeration)

The server takes the secrets listed for `name` (up to 4 during rotation) and pads the list to exactly 4 with freshly generated random secrets. Unknown names get 4 random secrets. It tries all 4 as the responder PSK, always all 4, and keeps the first that decrypts msg1. If none does, it closes the TCP connection without sending a byte. An unknown name and a wrong secret produce the same bytes (none), the same close, and the same work, so names cannot be enumerated. `tests/security.rs` measures the medians (ratio about 1.08 on loopback).

### Where each side is proven

| step                                    | what is proven                                                                                          | to whom                                      |
| --------------------------------------- | ------------------------------------------------------------------------------------------------------- | -------------------------------------------- |
| msg1 tag verifies                       | the sender knows the PSK for `name` (and agrees on the prologue)                                        | server. **Not fresh**: msg1 can be replayed. |
| msg2 tag verifies                       | the responder knows the PSK **and** saw this client's fresh `e` (`ee` is mixed in)                      | client: the server is authenticated and live |
| first transport record (hello) decrypts | the initiator holds the keys derived from the server's fresh `e`; a replayer of msg1 cannot derive them | server: the client is authenticated and live |

The server allocates no per-client state (session, pool entry) until the hello decrypts. A replayed msg1 gets a msg2 back and then times out (`limits.handshake_timeout_secs`). Forward secrecy comes from the ephemeral-ephemeral DH: recording traffic and later learning the PSK does not reveal past sessions. A captured handshake does allow **offline guessing** of the PSK, which is why the secret must be random and 256 bits (never a password).

## 5. Records

```
record = len:u16 || ciphertext[len]        16 <= len <= 65535
```

The ciphertext is a Noise transport message (plaintext + 16-byte tag). Each direction keeps its own 64-bit nonce counter starting at 0 (snow `StatelessTransportState`), so records cannot be dropped, reordered or replayed without failing authentication. A record shorter than 16 bytes, or one that fails to decrypt, ends the channel.

**End of stream** is a record with an empty plaintext (`len = 16`). A TCP close without that record is treated as truncation, an error, not a clean EOF.

## 6. Hello (client → server, first record)

| type    | bytes                     |
| ------- | ------------------------- |
| control | `01 version:u8`           |
| data    | `02 version:u8 token[16]` |

On the server the hello is read with a fixed 256-byte limit. `version` is 1. `token` is the session token from the current control channel's Welcome. A data channel with an unknown or stale token is closed.

## 7. Control messages

Server → client (one message per record):

| id  | message  | body                                                                             |
| --- | -------- | -------------------------------------------------------------------------------- |
| 1   | Welcome  | `token[16] heartbeat_secs:u16`                                                   |
| 2   | Services | `count:u16` then per service `name:str8 protocol:u8 bind_port:u16 target:str8`   |
| 3   | Ping     | (none)                                                                           |
| 4   | OpenData | `count:u16`: open this many data channels                                        |
| 5   | Bye      | `reason:u8` (1 shutdown, 2 replaced, 3 removed, 4 timeout, 5 plaintext mismatch) |

Client → server: `0x81` Pong.

`str8` = `len:u8 bytes`. Unknown ids, truncated bodies and trailing bytes are errors.

Heartbeat: the server sends Ping every `heartbeat.interval_secs`, and the client answers Pong. Each side drops the channel after 3 intervals without receiving anything. The client learns the interval from Welcome, so the two sides cannot disagree.

A second control channel for the same name replaces the first (`Bye replaced`). If `insecure_plaintext` differs between the two sides, the server authenticates first and then sends `Bye plaintext mismatch`, so the client can print an exact reason.

## 8. Data channels

The pool is per client. The server keeps `target` idle channels: it starts at `pool.min_idle`, doubles (up to `pool.max_idle`) whenever a visitor finds the pool empty, and decays by one per idle minute. It asks for the missing ones with OpenData. Idle channels whose peer has closed are dropped before use.

While a channel is idle in the pool, the server watches it: any data or close from the client discards it. On a WebSocket carrier, the server also sends a keepalive record (plaintext `0x00`) every `websocket.keepalive_secs` (default 25 s), so proxies with idle timeouts keep the connection open. The client ignores keepalive records before Connect, and on a WebSocket carrier it closes a channel that receives nothing for 90 s.

When a visitor connects to a service, the server takes an idle channel and sends:

```
Connect = 10 protocol:u8 service:str8 target:str8      (protocol 0 tcp, 1 udp)
```

The client resolves `target`, keeps only the addresses permitted by its allow list (checked after DNS, and only those addresses are dialed), connects, and replies with one record:

```
Status = 0 ok | 1 denied by allow list | 2 unreachable | 3 unresolved | 4 invalid
```

The server does not wait for Status before sending visitor bytes, so connection setup adds no round trip. If Status is not 0, the server logs the reason and closes the visitor.

**TCP**: after Connect/Status both directions carry stream records, each ending with the end-of-stream record. Half-close is preserved: the EOF record makes the receiver `shutdown(Write)` its socket while the other direction keeps flowing.

**UDP**: each record is `peer:u32 || datagram` (at most 65515 bytes). The server assigns a peer id per visitor address. The client keeps one connected UDP socket per peer id. Both sides expire peers after `udp.session_timeout_secs` of silence, with at most 4096 peers per service.

**insecure_plaintext**: the handshake, hello, Connect and Status are still encrypted records. After them, TCP data channels carry raw bytes (TCP FIN carries half-close). UDP stays encrypted.

## 9. Limits enforced before authentication

- at most `limits.max_handshakes` connections in steps 2–5 at once; extra connections are closed immediately;
- the whole of steps 2–5 must finish within `limits.handshake_timeout_secs`;
- every read before authentication has a fixed maximum size: on WebSocket, an 8 KiB request head; then 7, 64 + 48, 2 and 256 bytes.
