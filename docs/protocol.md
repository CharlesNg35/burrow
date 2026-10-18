# burrow wire protocol, version 1

All integers are big-endian. "Record" means one Noise transport message with a 2-byte length prefix.

## 1. Channels

A client opens TCP connections to the server's `listen` address. Each connection is either:

- a **control channel**: one per client, long-lived, carries commands and heartbeats;
- a **data channel**: one per forwarded TCP connection, or one per UDP service; opened ahead of time into the server's pool.

A visitor (`burrow visit`) opens a third kind:

- a **visit channel**: one per local TCP connection, relayed by the server to a data channel of the client (section 9).

Every channel, control or data, does its own full Noise handshake.

## 2. Carriers

The byte stream below can be carried in two ways. Both carry exactly the same bytes.

- **TCP** (`listen`): the stream is the TCP connection.
- **WebSocket** (`websocket`, opt-in): the client sends an HTTP/1.1 `GET <path>` upgrade request (RFC 6455), optionally over TLS (`wss://`), with any extra `headers` from its config. Before upgrading, the server reads at most 8 KiB of request head and answers `404` for any other path, `426` for a non-upgrade request on the path, and `400` or `431` for malformed or oversized heads. After the upgrade, the stream is the concatenation of the payloads of binary messages, in order. The sender batches records into messages of up to 128 KiB, and larger messages are rejected. Text, ping and pong messages carry no stream data, more than 100 of them within 10 s end the connection, as do more than 100 pings in a row while the peer reads none of the pongs, and a close message ends the stream. Authentication is still the Noise handshake below: the URL, path and headers are not secrets.

On a WebSocket carrier, data channels always use encrypted records, even with `insecure_plaintext`, because WebSocket has no half-close for the raw-stream mode in section 8.

The WebSocket upgrade and the Noise handshake share one `limits.handshake_timeout_secs` deadline and one `limits.max_handshakes` slot.

## 3. Preamble (client → server, clear text)

| field    | size | value                                                                             |
| -------- | ---- | --------------------------------------------------------------------------------- |
| magic    | 4    | `BRW1`                                                                            |
| kind     | 1    | `1` control, `2` data, `3` visit                                                  |
| flags    | 1    | bit 0: AES-GCM (else ChaChaPoly); bit 1: insecure_plaintext; other bits must be 0 |
| name_len | 1    | 1..=64                                                                            |
| name     | n    | client name, or visitor name for kind 3, `[A-Za-z0-9._-]`                         |
| msg1     | 48   | Noise message 1                                                                   |

The server reads exactly 7 bytes, validates them, then reads exactly `name_len + 48` bytes into a fixed stack buffer. Nothing the peer sends before authentication controls an allocation size, except that over WebSocket the message carrying these bytes is buffered up to its 128 KiB limit.

## 4. Noise handshake

- Pattern: `Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s`, or `Noise_NNpsk0_25519_AESGCM_BLAKE2s` when flag bit 0 is set (snow 0.10, `Builder::psk(0, key)`).
- Message tokens (Noise spec section 9.4): `-> psk, e` then `<- e, ee`.
- PSK: the client's 32-byte secret (`clients.<name>.secret`). The spec requires 256 bits of entropy; burrow rejects anything that does not decode to exactly 32 bytes, or that has fewer than 16 distinct byte values.
- Prologue: `"burrow/1\0" || preamble-without-msg1`. Kind, flags and name are therefore authenticated: changing any of them makes message 1 fail to decrypt.
- msg1 = 32-byte ephemeral `e` + 16-byte tag of the empty payload. Because the `psk` token runs first, the payload is encrypted with a key derived from the PSK (spec section 9.2: in PSK handshakes `e` is followed by `MixKey(e)`).
- msg2 = 32-byte ephemeral `e` + 16-byte tag (empty payload), sent server → client.

### Server-side secret lookup (anti-enumeration)

The server takes the secrets listed for `name` (up to 4 during rotation), under `clients` for control and data channels and under `visitors` for visit channels, and pads the list to exactly 4 with freshly generated random secrets. Unknown names get 4 random secrets. It tries all 4 as the responder PSK, always all 4, and keeps the first that decrypts msg1. If none does, it sends the single byte `0x00` and closes the connection. An unknown name and a wrong secret produce the same byte, the same close, and the same work, so names cannot be enumerated. A client counts only this byte as a refusal: a connection that closes before msg2 without it (a busy server, a timeout, a forwarder whose backend is down) is a lost connection and is retried. `tests/security.rs` measures the medians (ratio about 1.08 on loopback).

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

| type    | bytes                        |
| ------- | ---------------------------- |
| control | `01 version:u8 instance[16]` |
| data    | `02 version:u8 token[16]`    |
| visit   | `03 version:u8`              |

The hello type must match the preamble kind; any other pairing is closed. On the server the hello is read with a fixed 256-byte limit. `version` is 1. `instance` is random and stays the same for the life of a client process. `token` is the session token from the current control channel's Welcome. A data channel with an unknown or stale token gets one record `0x01` and is closed; a client that gets this on three data channels in a row reconnects, since its server's address may now lead to another server.

## 7. Control messages

Server → client (one message per record):

| id  | message  | body                                                                                          |
| --- | -------- | --------------------------------------------------------------------------------------------- |
| 1   | Welcome  | `token[16] heartbeat_secs:u16`                                                                |
| 2   | Services | `count:u16` then per service `name:str8 protocol:u8 bind_port:u16 target:str8 ports:u16`      |
| 3   | Ping     | (none)                                                                                        |
| 4   | OpenData | `count:u16`: open this many data channels                                                     |
| 5   | Bye      | `reason:u8` (1 shutdown, 2 replaced, 3 removed, 4 timeout, 5 plaintext mismatch, 6 duplicate) |

Client → server:

| id     | message | body                                                                                    |
| ------ | ------- | --------------------------------------------------------------------------------------- |
| `0x81` | Pong    | (none)                                                                                  |
| `0x82` | Allow   | `count:u8` (at most 64) then `rule:str8` each: the client's `allow` list, `any` for all |

A client sends Allow once after Welcome; the server keeps it for the session and shows it only to visitors with `reach` for that client (see section 9).

`str8` = `len:u8 bytes`. Unknown ids, truncated bodies and trailing bytes are errors for the client; the server ignores a client message it cannot decode, which still counts as a sign of life.

Heartbeat: the server sends Ping every `heartbeat.interval_secs`, and the client answers Pong. Each side drops the channel after 3 intervals without receiving anything. The client learns the interval from Welcome, so the two sides cannot disagree.

A second control channel for the same name with the same `instance` replaces the first (`Bye replaced`), so a client that reconnects is not kept out by its own stale session. One with another `instance` is refused with `Bye duplicate` while the first is connected, so two machines with the same config do not take the name from each other. If `insecure_plaintext` differs between the two sides, the server authenticates first and then sends `Bye plaintext mismatch`, so the client can print an exact reason.

## 8. Data channels

The pool is per client. The server keeps `target` idle channels: it starts at `pool.min_idle`, doubles (up to `pool.max_idle`) whenever a connection finds the pool empty, and decays by one per idle minute. It asks for the missing ones with OpenData. Idle channels whose peer has closed are dropped before use.

While a channel is idle in the pool, the server watches it: any data or close from the client discards it. On a WebSocket carrier, the server also sends a keepalive record (plaintext `0x00`) every `websocket.keepalive_secs` (default 25 s), so proxies with idle timeouts keep the connection open. The client ignores keepalive records before Connect, and on a WebSocket carrier it closes a channel that receives nothing for 90 s.

When a connection arrives at a service, the server takes an idle channel and sends:

```
Connect = 0x10 protocol:u8 service:str8 target:str8 only:u8 [count:u8 rule:str8*]      (protocol 0 tcp, 1 udp)
```

The client resolves `target`, keeps only the addresses permitted by its allow list (checked after DNS, and only those addresses are dialed), connects, and replies with one record:

```
Status = 0 ok | 1 denied by allow list or reach rules | 2 unreachable | 3 unresolved | 4 invalid
         | 5 forbidden | 6 offline | 7 busy      (5-6 only from the server to a visitor; 7 also from a client at its connection limit: 16384, or less where the open file limit is lower)
```

The server does not wait for Status before sending the connection's bytes, so connection setup adds no round trip. If Status is not 0, or none arrives within 3 × the heartbeat interval, the server logs the reason; for a service with several clients it moves the connection to the next one, sending again the PROXY header and up to 64 KiB of bytes it had already sent, and otherwise closes the connection.

`target` is `host:port`, or `unix:/absolute/path` for a Unix socket, which the client checks against the `unix:` entries of its allow list. For a port range the server sends the target port that matches the public port the connection arrived on; Services carries the first target port in `target` and the size of the range in `ports`.

With `proxy_protocol`, the first bytes the server sends after Connect are a PROXY protocol v1 or v2 header with the real source address and the public address it connected to (for a visit, the visitor's address as the server sees it). The client forwards them like any other data, so it needs no part in it.

With `health_check`, the server also opens connections of its own: every interval it sends Connect for the service's first target port on each client, reads Status, and ends the channel at once. Status 0 counts as up; other answers and timeouts count as failures.

**TCP**: after Connect/Status both directions carry stream records, each ending with the end-of-stream record. Half-close is preserved: the EOF record makes the receiver `shutdown(Write)` its socket while the other direction keeps flowing.

**UDP**: each record is `peer:u32 || datagram` (at most 65515 bytes). The server assigns a peer id per sender address. The client keeps one connected UDP socket per peer id. Both sides expire peers after `udp.session_timeout_secs` of silence, with at most 4096 peers per service port. Each port of a UDP range keeps one data channel while its relay runs.

**insecure_plaintext**: the handshake, hello, Connect and Status are still encrypted records. After them, TCP data channels carry raw bytes (TCP FIN carries half-close). UDP stays encrypted.

## 9. Visit channels

After the hello, the visitor sends one request record without waiting for a reply:

```
VisitRequest = 0x20 check:u8 kind:u8 ...
  kind 1: service:str8 index:u16     a service that lists this visitor in "visitors";
                                     index picks a port of a port range, 0 for one port
  kind 2: client:str8 target:str8    any target on a client, if the visitor has reach for it
```

Names follow the rules of section 3. The server reads the request, a record of at most 512 bytes, within the same `limits.handshake_timeout_secs` deadline and while it still counts against `limits.max_handshakes`, then checks it against its current config:

- the visitor still exists and still has the secret it authenticated with;
- for kind 1, the service exists, is TCP, lists the visitor, admits the visitor's address in `allow_from`/`deny_from`, and `index` is inside its range (`4 invalid` otherwise);
- for kind 2, the visitor has `reach` for the client, and the target is a valid `host:port` of at most 255 bytes, or `unix:/absolute/path`, or empty in a check, which asks only whether the visitor may reach that client (a SOCKS forward checks this way); if that `reach` has rules, an IP address or socket path target must match one of them.

Unknown services and services that do not list the visitor get the same answer, `5 forbidden`, so names cannot be probed. Then the server checks that the client is connected (`6 offline`). With `check = 1` it answers `0 ok` at this point and closes: `burrow visit` uses this at startup to report problems before the first connection. Otherwise it checks the limits of the visitor, the service and the client (`7 busy`).

Otherwise the server takes a data channel of the client (for a service with several clients, the one chosen as for public connections) and sends it a Connect (section 8) with the service name, or an empty name for kind 2, and the target. For kind 2 with `reach` rules, `only` is 1 and the rules follow; the client then connects only to addresses, after resolving a name on its own network, that the rules match, and denies the connection when none do or a rule cannot be read. The client applies its allow list as for any other connection and replies with Status, which the server passes on to the visitor. The visitor waits for this Status before sending data, so a refusal always arrives with its reason. From then on, the server decrypts each record from one channel and encrypts it for the other, both ways, until each side has sent its end-of-stream record. With `insecure_plaintext`, only the leg between server and client is raw; the visit channel is always encrypted.

On every config reload, the server checks the request again and closes the connection when it is no longer granted: the visitor, its secret, its place in `visitors` or its `reach` for the client is gone, the service no longer runs on that client, or a `reach` rule change no longer covers the target (for a host name target, any change of those rules, since only the client resolves it). A changed service target, like a widened `reach`, leaves open connections running.

Instead of a VisitRequest, the visitor may send the single byte `0x21` to ask what it may use. The server checks the visitor and its secret against its current config, answers `5 forbidden` if they no longer match, and otherwise sends one record and closes:

```
Grants = 0x22 services:u16 (name:str8 port:u16 ports:u16)* clients:u16 (name:str8 count:u8 reach:str8* known:u8 [count:u8 rule:str8*])*
```

`services` are the TCP services that list the visitor, wherever it asks from (`allow_from`/`deny_from` apply when it connects), with the target's port (0 for a Unix socket) and the size of their range (at most 1000); `clients` are the clients in its `reach`, each with the visitor's rules for it (none for `any`) and, for `any` only, the Allow list of their current session when `known` is 1. If the answer does not fit in one record, the server leaves out the Allow lists, and answers `4 invalid` if it still does not fit. `burrow init visitor` uses it to write the forwards, and `--update` to refresh them.

A visitor's SOCKS5 forward reads the target from the SOCKS request (CONNECT only, no authentication, IPv4, IPv6 or a host name) and sends it as kind 2. It answers the SOCKS client only after the server's Status, mapping refusals to SOCKS reply codes.

## 10. Limits enforced before authentication

- at most `limits.max_handshakes` connections in steps 2–5 at once; once fewer than half of them are free, also at most an eighth (at least 8) from one IPv4 address or IPv6 /64 other than loopback; extra connections are closed immediately;
- the whole of steps 2–5 must finish within `limits.handshake_timeout_secs`;
- every read before authentication has a fixed maximum size: on WebSocket, an 8 KiB request head and messages of at most 128 KiB; then 7, 64 + 48, 2 and 256 bytes, and 512 bytes for a visit request.
