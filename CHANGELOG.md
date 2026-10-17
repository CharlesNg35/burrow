# Changelog

## [0.2.0]

- **Private services and visitors.** A service without `bind` opens no port on the server; the visitors it lists reach it from their own machine with `burrow visit`, through the server and the client's tunnel. `reach: { CLIENT: any }` lets a visitor pick any target that client allows, like `ssh -L`. The client's `allow` list still decides, and reload closes the open connections of a visitor whose access was removed.
- New commands: `burrow visit` (all forwards, one with `visit NAME --local PORT`, or ad hoc with `--service` or `--client --target`), `burrow visitor-config` on the server, and `burrow init visitor`. `burrow service install` runs visitor configs too.
- `validate` checks visitors: unknown names in `visitors` and `reach`, private services nobody can reach, duplicate local ports, `0.0.0.0` forwards, and a warning for every `reach`.
- The server refuses a hello that does not match its channel kind, and connection limits of a service now count across reloads that move its port.
- **`burrow update`** downloads the latest release (or `--to VERSION`), verifies it against `SHA256SUMS` and by running it, then replaces this binary and updates the installed services with rollback. `--check` only reports whether a newer release exists. It honours `HTTPS_PROXY` and `NO_PROXY`.
- Fix: a WebSocket data channel now ends with a TLS `close_notify` (`wss://`) and a clean FIN. Before, a TLS proxy in front of the server could treat the abrupt end as an error and drop the last up to 128 KB of a stream under load.
- **Port ranges**: `bind`, `target` and a visitor's `local` take ranges like `30000-30010`; each port reaches the same position in the target range.
- **Several clients per service**: `clients: [a, b]` with `mode: failover` or `balance`. A connection that a client cannot serve, because its target is down or not in its `allow` list, moves to the next client, and that client is skipped for a while (a refusal until it reconnects). `health_check` also checks the target through each client ahead of time.
- **Per-service controls**: `allow_from` and `deny_from` for source addresses (checked again on reload, also for visitors), `proxy_protocol: v1 | v2` to send the real client address to the target, and `bandwidth` to cap each direction.
- **Unix socket targets**: `target: unix:/path`, permitted on the client by `unix:` entries in `allow`.
- **SOCKS5 for visitors**: `burrow visit --client home --socks 1080`, or `socks: true` on a forward; `visitor-config` writes one for each client in `reach`.
- **`burrow status`** shows every server running on the machine, however it was started, with no setup and nothing reachable from the network: tables with color in a terminal that fit its width, `-f` for a live screen with rates, `--json` for scripts. Prometheus metrics stay optional (`metrics.listen`, off by default).
- **`burrow edit`** finds the configs in this folder, of installed services and of running instances, opens one in your editor (`--editor`, `VISUAL` or `EDITOR`), checks it like `validate`, saves it with its owner and permissions only if nobody changed it meanwhile, and applies it: a server reloads by itself, a client or visitor service is restarted after asking.
- A renewed `wss://` certificate is picked up from its files without a restart.
- Fix: `update`, `service update`, `service restart`, `service reload` and reinstalling a running service finish even when the SSH session that started them runs through the tunnel and drops; before, the services could stay stopped.
- Fix: a connection on a data channel that went silent ends after three heartbeats instead of about 15 minutes; a burst of UDP peers no longer grows the client's memory or exhausts its open files; a reload no longer stalls on ports taken by other programs; failed accepts are logged once per 10 seconds.
- Fix: UDP datagrams larger than 9 KB are forwarded on macOS too; its default send buffer dropped them.
- Fix: uninstalling the last service over SSH on macOS, which blocks deleting accounts there, now removes everything else and says how to delete the account; unused instance locks are removed too.
- Visitors and client data channels retry a handshake that a busy server dropped, for up to 5 s, instead of failing the connection; a server that is down still fails at once.
- Protocol: a visit channel kind, request and statuses (docs/protocol.md section 9) and a port index in visit requests. The service list keeps its 0.1.0 format, so 0.1.0 clients work with a 0.2.0 server, and 0.2.0 clients with a 0.1.0 server.

## [0.1.0]

First release.

- Reverse TCP and UDP tunnel. The server config decides what is exposed, and services can be added, changed or removed with a hot reload (file watch or SIGHUP), without restarting the client.
- Noise `NNpsk0` (ChaChaPoly by default, AES-GCM optional) with a separate 256-bit secret per client, secret rotation, constant-work rejection of unknown names, and a client-side allow list that is deny-by-default.
- Adaptive data-channel pool per client, heartbeat, half-close, HTTP and SOCKS5 proxy support for the client.
- Optional WebSocket transport (`ws://` behind a TLS proxy, or `wss://` with your own certificate) for servers that are only reachable over HTTP(S). It is off unless configured, and `init server --ws` writes a config without a TCP control port.
- Clients never give up by default: jittered backoff, DNS re-resolved on every attempt, Happy Eyeballs dialing, one outage summary per minute, and optional `give_up_after_secs` / `on_rejected: exit`.
- `burrow validate` reports every config problem with file:line:column, a caret, a fix and "did you mean" suggestions. The same checks run when the server or client starts and on every reload.
- `burrow init`, `client-config`, `gensecret`, `schema` and `completions`. `client-config` takes the client address from `--server` or the server's optional `public_address` and never guesses it from `listen`; a host alone gets its port from `listen`. The client name can be left out when there is only one, `-o FILE` writes a file only you can read, and the format follows the `-o` extension.
- Short forms in config files: `listen: 7835`, `bind: 7840` and `target: 3000` (for `127.0.0.1:3000`), and `server: host` for port 7835.
- `burrow service` installs a server or client as a system service on Linux (systemd, sandboxed), macOS (launchd) and Windows (SCM), with a dedicated account, a copied config and binary, a port check before install, rotating log files (`--log-file`), `status`, `logs -f`, `list`, and `update` with rollback.
- The same tunnel cannot run twice (an OS file lock), and `-c` can be left out when `server.yaml` or `client.yaml` is in the current folder.
- Static Linux binaries (amd64, arm64), macOS and Windows builds, and a multi-arch distroless image.
