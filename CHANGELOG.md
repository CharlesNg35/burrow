# Changelog

## [0.1.0]

First release.

- Reverse TCP and UDP tunnel. The server config decides what is exposed, and services can be added, changed or removed with a hot reload (file watch or SIGHUP), without restarting the client.
- Noise `NNpsk0` (ChaChaPoly by default, AES-GCM optional) with a separate 256-bit secret per client, secret rotation, constant-work rejection of unknown names, and a client-side allow list that is deny-by-default.
- Adaptive data-channel pool per client, heartbeat, reconnect with jittered backoff, half-close, HTTP and SOCKS5 proxy support for the client.
- `burrow validate` reports every config problem with file:line:column, a caret, a fix and "did you mean" suggestions. The same checks run when the server or client starts and on every reload.
- `burrow init`, `client-config`, `gensecret`, `schema` and `completions`.
- Static Linux binaries (amd64, arm64), macOS and Windows builds, and a multi-arch distroless image.
