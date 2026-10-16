# Changelog

## [0.1.0]

First release.

- Reverse TCP and UDP tunnel. The server config decides what is exposed, and services can be added, changed or removed with a hot reload (file watch or SIGHUP), without restarting the client.
- Noise `NNpsk0` (ChaChaPoly by default, AES-GCM optional) with a separate 256-bit secret per client, secret rotation, constant-work rejection of unknown names, and a client-side allow list that is deny-by-default.
- Adaptive data-channel pool per client, heartbeat, half-close, HTTP and SOCKS5 proxy support for the client.
- Optional WebSocket transport (`ws://` behind a TLS proxy, or `wss://` with your own certificate) for servers that are only reachable over HTTP(S). It is off unless configured, and `init server --ws` writes a config without a TCP control port.
- Clients never give up by default: jittered backoff, DNS re-resolved on every attempt, Happy Eyeballs dialing, one outage summary per minute, and optional `give_up_after_secs` / `on_rejected: exit`.
- `burrow validate` reports every config problem with file:line:column, a caret, a fix and "did you mean" suggestions. The same checks run when the server or client starts and on every reload.
- `burrow init`, `client-config`, `gensecret`, `schema` and `completions`.
- `burrow service` installs a server or client as a system service on Linux (systemd, sandboxed), macOS (launchd) and Windows (SCM), with a dedicated account, a copied config and binary, a port check before install, rotating log files (`--log-file`), `status`, `logs -f`, `list`, and `update` with rollback.
- The same tunnel cannot run twice (an OS file lock), and `-c` can be left out when `server.yaml` or `client.yaml` is in the current folder.
- Static Linux binaries (amd64, arm64), macOS and Windows builds, and a multi-arch distroless image.
