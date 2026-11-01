# Monitoring

Monitoring tells you whether your clients are connected, which services carry traffic, and whether every certificate is fine, before a user notices a problem. This page also covers running burrow at large scale, and its logs. There are three ways to see what burrow is doing:

- **`burrow status`** on the server machine: clients, services, visitors and traffic, with no setup.
- **Prometheus metrics**, off unless you turn them on.
- **`burrow admin`**, a live dashboard in your browser. See [admin.md](admin.md).

For clients and visitors, use `burrow service status` and `burrow service logs -f` ([service.md](service.md)).

## burrow status

`burrow status` shows every burrow server running on this machine, however it was started: by hand, by your own systemd unit, or with `burrow service install`. It needs no config and no metrics, and opens nothing on the network.

Run it on the server machine:

```sh
sudo burrow status                # every server on this machine
sudo burrow status burrow-server  # only this installed service
burrow status -c server.yaml      # the server running this config
burrow status -f                  # refresh every second, with connections/s and bytes/s, until Ctrl-C
burrow status -f --interval 5     # refresh every 5 seconds
burrow status --json              # the raw data, for scripts
```

A server installed as a service needs `sudo` (an administrator terminal on Windows). A server that does not answer is shown as not answering, not as stopped. The output looks like this:

```
$ sudo burrow status
burrow 0.2.0  ·  up 3 d 4 h  ·  clients on tcp 0.0.0.0:7835

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

With the web proxy on, `burrow status` also lists the websites and the certificates burrow serves:

```text
WEBSITES
  NAME                  KIND                     SERVICE
  blog.example.com      HTTPS                    blog
  dash.lan.example.com  plain HTTP, unencrypted  dashboard

CERTIFICATES
  NAME              SOURCE                    STATE    EXPIRES   NEXT RENEWAL
  blog.example.com  Let's Encrypt, TLS check  ok       in 59 d   in 29 d
Certificate orders, one at a time:
  wiki (wiki.example.com): next try 25 Oct 2026 14:05 UTC
Let's Encrypt account https://acme-v02.api.letsencrypt.org/acme/acct/123, kept in /var/lib/burrow/home
```

A failing certificate gets a line with the reason, and a link when Let's Encrypt gives one. The admin UI shows the same in the Certificates tab of its Overview, and lists failing, expired and soon expiring certificates under Needs attention. See [When a certificate fails](web/certificates.md#when-a-certificate-fails) for what to check.

<details>
<summary><b>What the columns mean</b></summary>

- **OPEN:** connections open now. **TOTAL:** connections since the server started.
- **IDLE:** channels the client keeps open in advance, so a new connection starts at once.
- **at limit** after a client's state: in the last minute the client refused new connections because it reached its `max_connections`. In `--json`, `at_limit` is true, and `busy_total` counts those refusals since the server started.
- **REFUSED:** connections the service refused: a blocked source address, a limit (the service's own, or a client that answered busy at its `max_connections`), or no client to serve it.
- **IN:** bytes from users to the target. **OUT:** bytes back to users.
- **LAST USED** of a visitor: `now` while it has connections open. Otherwise, how long ago it last opened or closed one, or a running `burrow visit` or `burrow init visitor` last checked in (`2 min ago`); or `not since start` when it has not used the server since the server started. In `--json` it is `last_seen_secs`: the seconds since then, or `null` when not since start.
- **CLIENTS** of a service: whether each client can serve it now. `failing` or `refused` means the client could not reach the target or its `allow` list refused it, so it is skipped for a while.
- **refused handshakes:** connections that failed to log in, because of a wrong secret, an unknown name or a limit.

</details>

## Prometheus metrics

[Prometheus](https://prometheus.io/) collects numbers from your servers over time, for graphs and alerts. burrow's metrics are off unless you turn them on in server.yaml:

```yaml
# server.yaml
metrics:
  listen: 127.0.0.1:9100   # a port alone means 127.0.0.1
```

The server then serves metrics at `http://127.0.0.1:9100/metrics`, in the format Prometheus reads. Point Prometheus at it:

```yaml
# prometheus.yml
scrape_configs:
  - job_name: burrow
    static_configs:
      - targets: ["127.0.0.1:9100"]
```

Let's Encrypt no longer sends expiry emails, so alert on certificates yourself, for example when `burrow_certificate_expiry_timestamp_seconds - time()` drops below 14 days.

Keep `listen` on `127.0.0.1` unless a firewall limits who can connect. Anyone who reaches it sees the names of your clients, services and visitors, and their traffic.

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
| `burrow_client_busy_total` | counter | `client` | connections the client answered busy because it reached its `max_connections` |
| `burrow_service_connections_open`, `_total` | gauge, counter | `service` | |
| `burrow_service_refused_total` | counter | `service` | source address, limits, or no client |
| `burrow_service_bytes_total` | counter | `service`, `direction` | `in` from users to the target, `out` back |
| `burrow_web_responses_total` | counter | `service`, `class` | HTTP responses a website sent since the server started, by status class (`1xx` to `5xx`), including burrow's own pages such as 404, 413, 502 and maintenance. Only for services with `web:` that burrow terminates or serves as plain HTTP; a `tls: passthrough` site has none, because burrow cannot see its requests. Bytes are in `burrow_service_bytes_total`, as for every service. `burrow status` shows the same counts under each website, like `web responses 2xx 1203, 4xx 17`. |
| `burrow_service_client_up` | gauge | `service`, `client` | health check result, with `health_check` only |
| `burrow_visitor_connections_open`, `_total` | gauge, counter | `visitor` | |
| `burrow_certificate_expiry_timestamp_seconds` | gauge | `name` | when the certificate served for a web name expires, in Unix seconds |
| `burrow_certificate_failures_total` | counter | `name` | failed tries to get or renew a web name's certificate since the server started |

</details>

## Running at scale

Past tens of thousands of connections, the operating system's limits matter more than burrow's. Measured on Linux with idle connections:

| | memory per idle connection | 100,000 idle connections |
| --- | --- | --- |
| server | about 9 KB | 895 MiB |
| clients | about 15 KB | 1.5 GiB across 8 clients |

The 100,000 connections opened in 5 seconds.

Busy connections cost more. In a load test of websites over HTTPS, the server held about 70 KB per open or waiting connection (1,448 MB at 20,521 open files, 1,078 MB at 14,701) and was back to about 150 MB 3 minutes after the load. Size the machine's memory, and any `MemoryMax=` or container memory limit, for the peak number of connections at about 70 KB each plus about 50 MB: a limit that is too small can get burrow killed during a pile-up of slow connections, as a 512 MB `MemoryMax` did at about 7,900 open files. When its clients cannot open tunnel channels fast enough, burrow answers the waiting requests with a 503 "busy" page instead of holding them without end.

**Limits per client.** A client keeps at most `max_connections` (16384) connections and `max_udp_peers` (8192) UDP peers open at once. The server keeps at most `limits.max_connections_per_client` (8192) connections per client. Beyond these limits, new connections are refused and datagrams from new UDP peers are dropped; open connections keep running. When a client refuses connections at its `max_connections`, `burrow status` shows it as `at limit`, and the client logs a warning at most once a minute.

Raise the limits in client.yaml and server.yaml; a saved file applies at once. Above about 28,000 connections through one client, also widen the local port range (see below):

```yaml
# client.yaml
max_connections: 50000
```

```yaml
# server.yaml
limits:
  max_connections_per_client: 50000
```

**Several clients.** For very large loads, run several clients for the same services, on one machine or several. The server spreads connections across them (see `mode` in [services.md](services.md)), and each client has its own limits.

**Linux limits.** These run out before burrow's memory does:

- **Connection tracking:** each tunnelled connection is 3 TCP connections: user to server, client to server, and client to the app. With the server and the client on one machine, that machine tracks all 3; on separate machines, each tracks about 2. The default table often holds 262144. Check and raise it:

  ```
  sysctl net.netfilter.nf_conntrack_max
  sudo sysctl -w net.netfilter.nf_conntrack_max=1048576
  ```

  When it is full, the kernel logs "nf_conntrack: table full, dropping packet" and new connections time out.
- **Local ports:** a machine opens about 28,000 connections from one source address to one address and port. Every connection through a client also needs a channel from the client to the server's address. So one client keeps at most about 28,000 connections open, whatever their targets. Widen the range, or spread the load over several clients on other machines or with other server addresses:

  ```
  sysctl net.ipv4.ip_local_port_range
  sudo sysctl -w net.ipv4.ip_local_port_range="1024 65535"
  ```

- **Open files:** every connection uses open files on the server and the client. burrow raises its own limit to the hard limit at start, so the hard limit is what counts. `burrow service install` sets 1048576 on Linux. For a burrow you run by hand, raising the hard limit needs root: set `nofile` in `/etc/security/limits.conf`, or `LimitNOFILE=` in your own systemd unit. If the limit is too small, the client lowers `max_connections` and `max_udp_peers` to fit, and logs a warning when it lowers a value you set.

To keep a `sysctl -w` change after a reboot, put the same line, without `sudo sysctl -w`, in a file such as `/etc/sysctl.d/90-burrow.conf`.

## Logs

The log says what burrow does and why something failed. burrow logs to the terminal by default, and these options work with every command:

| option | meaning |
| --- | --- |
| `--log-level LEVEL` | `error`, `warn`, `info` (default), `debug` or `trace` |
| `-q`, `--quiet` | only warnings and errors |
| `--log-json` | one JSON object per line, for log collectors |
| `--log-file FILE` | write to this file, rotated at 10 MB with 5 old files kept |

An installed service writes its log to a file, and `burrow service logs -f` follows it ([service.md](service.md)).

## Next steps

- [When a certificate fails](web/certificates.md#when-a-certificate-fails): what to check.
- [burrow admin](admin.md): the same status, live, in your browser.
