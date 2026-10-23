# Monitoring

There are three ways to see what burrow is doing:

- **`burrow status`** on the server machine: clients, services, visitors and traffic, with no setup.
- **Prometheus metrics**, off unless you turn them on.
- **`burrow admin`**, a live dashboard in your browser. See [admin.md](admin.md).

For clients and visitors, use the service commands and logs: `burrow service status` and `burrow service logs -f`. See [service.md](service.md).

## burrow status

`burrow status` shows every burrow server running on this machine, however it was started: by hand, by your own systemd unit, or with `burrow service install`. It needs no config and no metrics, and opens nothing on the network.

```sh
sudo burrow status                # every server on this machine
sudo burrow status burrow-server  # only this installed service
burrow status -c server.yaml      # the server running this config
burrow status -f                  # refresh every second, with connections/s and bytes/s, until Ctrl-C
burrow status -f --interval 5     # refresh every 5 seconds
burrow status --json              # the raw data, for scripts
```

A server installed as a service needs `sudo` (an administrator terminal on Windows). A server that does not answer is named as such, not taken for stopped.

```
$ sudo burrow status
burrow 0.1.0  ·  up 3 d 4 h  ·  clients on tcp 0.0.0.0:7835

CLIENTS  1/2 connected
  NAME    STATE      SINCE           FROM            OPEN  IDLE
  home    connected  2 h 13 min      81.2.3.4:51234     3     4
  office  offline    left 5 min ago                     0     0

SERVICES
  NAME  PROTO  ADDRESS      TARGET          CLIENTS                  OPEN  TOTAL  REFUSED      IN     OUT
  web   tcp    0.0.0.0:443  127.0.0.1:8443  home up, office offline    12    153        1  1.2 GB  8.4 GB
  db    tcp    private      127.0.0.1:5432  home                        0      0        0     0 B     0 B
  web: balance, limit 5.0 MB/s, proxy protocol v2, source filter
  db: visitors laptop

VISITORS
  NAME    LAST USED        OPEN  TOTAL
  laptop  now                 1     12
  phone   2 min ago           0      3
  ops     not since start     0      0

2 refused handshakes since start (wrong secret, unknown name or limits)
```

In a terminal, the tables have borders and colors, and fit the window's width.

<details>
<summary><b>What the columns mean</b></summary>

- **OPEN:** connections open now. **TOTAL:** connections since the server started.
- **IDLE:** channels the client keeps open in advance, so a new connection starts at once.
- **REFUSED:** connections the service refused: a blocked source address, a limit, or no client to serve it.
- **IN:** bytes from users to the target. **OUT:** bytes back to users.
- **LAST USED** of a visitor: `now` while it has connections open, otherwise how long ago it last opened or closed one, or a running `burrow visit` or `burrow init visitor` last checked in (`2 min ago`), or `not since start` when it has not used the server since the server started. In `--json` it is `last_seen_secs`: the seconds since then, or `null` when not since start.
- **CLIENTS** of a service: whether each client can serve it now. `failing` or `refused` means the client could not reach the target or its `allow` list refused it, so it is skipped for a while.
- **refused handshakes:** connections that failed to log in, because of a wrong secret, an unknown name or a limit.

</details>

## Prometheus metrics

Metrics are off unless you turn them on in server.yaml:

```yaml
# server.yaml
metrics:
  listen: 127.0.0.1:9100   # a port alone means 127.0.0.1
```

The server then serves metrics at `http://127.0.0.1:9100/metrics`, in the format Prometheus reads:

```yaml
# prometheus.yml
scrape_configs:
  - job_name: burrow
    static_configs:
      - targets: ["127.0.0.1:9100"]
```

Keep `listen` on `127.0.0.1` unless a firewall limits who can connect: anyone who reaches it sees the names of your clients, services and visitors, and their traffic.

<details>
<summary><b>All metrics</b></summary>

| Metric | Type | Labels | |
| --- | --- | --- | --- |
| `burrow_info` | gauge | `version` | always 1 |
| `burrow_uptime_seconds` | gauge | | |
| `burrow_handshakes_refused_total` | counter | | connections refused before or during authentication |
| `burrow_udp_peers` | gauge | | UDP peers with an open session |
| `burrow_client_connected` | gauge | `client` | 1 when connected |
| `burrow_client_connections_open` | gauge | `client` | |
| `burrow_service_connections_open`, `_total` | gauge, counter | `service` | |
| `burrow_service_refused_total` | counter | `service` | source address, limits, or no client |
| `burrow_service_bytes_total` | counter | `service`, `direction` | `in` from users to the target, `out` back |
| `burrow_service_client_up` | gauge | `service`, `client` | health check result, with `health_check` only |
| `burrow_visitor_connections_open`, `_total` | gauge, counter | `visitor` | |

</details>

## Logs

burrow logs to the terminal by default. These options work with every command:

| option | meaning |
| --- | --- |
| `--log-level LEVEL` | `error`, `warn`, `info` (default), `debug` or `trace` |
| `-q`, `--quiet` | only warnings and errors |
| `--log-json` | one JSON object per line, for log collectors |
| `--log-file FILE` | write to this file, rotated at 10 MB with 5 old files kept |

An installed service writes its log to a file; `burrow service logs -f` follows it. See [service.md](service.md).
