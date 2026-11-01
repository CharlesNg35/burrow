# Web proxy benchmark: burrow, nginx and Caddy

This page compares burrow's web proxy with nginx and Caddy. All three ran in front of the same app, on the same machine, with the same certificate and load. It covers speed, CPU, memory and what happens under overload. The tunnel benchmark against rathole, frp and chisel is a separate page: [benchmark.md](benchmark.md).

Run on 2026-11-01, 18:31-21:36 UTC, with burrow 0.2.0 at commit `e3028e08`.

## Summary

- **burrow does better at:**
  - mixed traffic: its p99 at the same rate is 2.6-2.8 ms, against nginx's 45.5-45.7 ms and Caddy's 2.9-3.2 ms;
  - new TLS connections: it uses less CPU per handshake than nginx and Caddy;
  - memory: at the same rate it used the least memory on small requests and on the mix, and after a burst it used the least (34 MiB, against 91 for nginx and 1001 for Caddy);
  - overload: in the burst it was the only proxy with 0 errors in all three rounds.
- **burrow is behind on:**
  - small keep-alive requests: nginx holds 16-30 % more of them per second (56000 against 43200-48400), and at the same rate it uses 50-63 % of burrow's CPU;
  - CPU in general: nginx is the cheapest on keep-alive traffic, using 0.6-0.7 cores where burrow uses 1.1-1.2;
  - peak memory under heavy load: burrow's peak is higher than nginx's (up to 149 MiB against 92 MiB). It gives the extra memory back within a minute once the load stops.
- **Caddy** is behind both on small requests (26000-30800 per second) and in the burst. It held about 1 GiB of memory after the burst until new load came.

## Setup

| | |
|---|---|
| target (runs the proxies and the app) | KVM guest, Ubuntu 22.04.5, kernel 5.15, 6 vCPUs of an Intel Xeon Gold 5412U, 15 GB, Docker 29.2.1 |
| load (runs k6) | LXC container on the same Proxmox host, Ubuntu 25.04, 12 cores, 22 GB, k6 v2.3.0 |
| network | one 10 Gbit virtio bridge, 0.15 ms between the two |
| burrow | 0.2.0 at `e3028e08`, static musl build with jemalloc, as the release workflow builds it (`cargo zigbuild --locked --release --target x86_64-unknown-linux-musl`), sha256 `9555d3e7babcb95949325d06e91dd840d95913e297b71117265bfd38c4631121`. It runs in `alpine:3.22` with the binary mounted read-only |
| nginx | 1.30.6 (OpenSSL 3.5.7), official image `nginx@sha256:66b50d24288dba67f3d000b31223daefc4868c58d29aaacb6a4ff9771597b5eb` |
| Caddy | 2.11.7, official image `caddy@sha256:36c24ad91a7ff7bc4c87fe22bfa8df3c4b18da0773d73fc12ee2c52b98c0bb7e` |
| app | a small HTTP/1.1 server (Rust, hyper 1) on 127.0.0.1:19101 of the target, in its own container with 2 GB |

### Rules for a fair comparison

- **One proxy at a time:** each ran alone on port 8443, in Docker with `--network host`, `--memory 4g` and `--ulimit nofile=1048576`.
- **Same TLS for all three:** one ECDSA P-256 certificate, and k6 asked for TLS 1.3 only. Every answer was checked to be TLS 1.3, and all three negotiated `TLS_AES_128_GCM_SHA256`.
- **Same protocols:** visitors used HTTP/1.1 or HTTP/2, and the app was reached over HTTP/1.1 with keep-alive. k6 gives each virtual user its own connection, so "HTTP/2" here means many connections with one busy stream each, not a browser sharing one connection.
- **Same workers:** each used the 6 cores (nginx `worker_processes auto`, Caddy's Go default, burrow's tokio default).
- **Defaults otherwise:** access logs were off for all three, and every other setting was each project's documented default, except what the configs below say.
- **Key exchange (this favours nginx and Caddy):** burrow prefers the post-quantum hybrid X25519MLKEM768 ([D55](decisions.md#d55-tls-prefers-the-post-quantum-hybrid-x25519mlkem768-with-rustls-aws-lc-rs-provider)). k6 is a Go 1.26 client, which offers that hybrid first, so burrow used it with k6. nginx and Caddy were limited to X25519, P-256 and P-384 with `ssl_ecdh_curve` and `curves`, the same settings as the earlier runs. An `openssl s_client` offering `X25519MLKEM768:X25519` got the hybrid from burrow and X25519 from nginx and Caddy. A hybrid handshake costs more than X25519, so burrow did more work per new connection than the other two.

### Configs

```nginx
# nginx.conf
user nginx;
worker_processes auto;
worker_rlimit_nofile 65535;
error_log stderr warn;
pid /run/nginx.pid;

events {
    worker_connections 16384;     # the default 512 is too low for a load test
}

http {
    access_log off;
    keepalive_timeout 65s;
    client_max_body_size 16m;     # the default is 1m; the test uploads 1 MiB

    upstream app {
        server 127.0.0.1:19101;
        keepalive 32;             # the default since 1.29.7, written out
    }

    server {
        listen 8443 ssl;
        http2 on;
        server_name bench.lan;

        ssl_certificate     /conf/tls/cert.pem;
        ssl_certificate_key /conf/tls/key.pem;
        ssl_protocols TLSv1.2 TLSv1.3;
        ssl_ecdh_curve X25519:prime256v1:secp384r1;
        ssl_session_cache shared:SSL:10m;
        ssl_session_timeout 1d;
        ssl_session_tickets off;

        location / {
            proxy_pass http://app;
            proxy_set_header Host $host;
            proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
            proxy_set_header X-Forwarded-Proto $scheme;
        }
    }
}
```

```
# Caddyfile
{
	auto_https disable_redirects
	servers {
		protocols h1 h2              # no HTTP/3: burrow and nginx serve h1 and h2 here
	}
}

bench.lan:8443 {
	tls /conf/tls/cert.pem /conf/tls/key.pem {
		curves x25519 secp256r1 secp384r1
	}
	reverse_proxy 127.0.0.1:19101
}
```

```yaml
# server.yaml
listen: 127.0.0.1:17835 # required; no tunnel clients in the benchmark
web:
  https: 0.0.0.0:8443
services:
  app:
    client: self
    target: 19101
    web:
      hosts: [bench.lan]
      tls: { cert: tls/cert.pem, key: tls/key.pem }
```

## Workloads and method

| workload | what each request does | p99 limit |
|---|---|---|
| tiny h1 / tiny h2 | `GET /blob?size=100` over a kept-alive connection; the 100-byte body is checked by length and SHA-256 | 50 ms |
| mix h1 / mix h2 | a mix of requests: 70 % `GET /id`; 15 % downloads (80 % 16 KiB, 15 % 256 KiB, 4 % 1 MiB, 1 % 8 MiB), hashed; 5 % uploads (80 % 64 KiB, 20 % 1 MiB), hashed by the app | 500 ms |
| new TLS connection | tiny over HTTP/1.1, but every request on a new TCP and TLS connection with a full handshake | 50 ms |
| burst | mix over HTTP/1.1 at 18000/s and mix over HTTP/2 at 9000/s together for 60 s, then 120 s idle, then 60 s of mix h2 at 500/s | none: errors, slowdown and memory are recorded |

- **Every answer is checked:** k6 compares the status, the body length, the body hash and which app answered. A wrong answer counts even when the status is 200.
- **Fixed request rate:** k6 sends requests at a set rate (`constant-arrival-rate`), with at most 600 virtual users per k6. A request that cannot start because every virtual user is busy counts as "dropped".
- **Steps:** a step is 5 s of warm-up and 15 s measured. A step **holds** when the measured part has:
  - 0 errors and no wrong answer;
  - a p99 within the limit above;
  - at least 97 % of the asked rate answered;
  - at most 1 % of requests dropped.
- **Highest rate:** round 1 doubles the rate until a step fails, then halves the gap three times. Rounds 2 and 3 start from round 1's result and move in 10 % steps. The proxies take turns (burrow, nginx, Caddy, then again), and the tables show the median of 3 rounds with the lowest and highest in brackets.
- **Same rate for all three:** each workload ran for 30 s at half the lowest median highest rate, 3 rounds.
- **Burst:** each burst starts on a freshly started proxy, 3 rounds.
- **CPU and memory:** measured on the target every second, from each container's cgroup. CPU is in cores of 6. Memory is the PSS (proportional set size) summed over the container's processes, because nginx's 7 processes share pages. "Peak" is the highest one-second sample during the measured part.

## Results

### Highest rate that holds

Requests per second, median of 3 rounds (lowest-highest).

| workload | burrow | nginx | Caddy |
|---|---|---|---|
| tiny h1 | 43200 (43200-48000) | **56000** (56000-56000) | 30800 (28000-30800) |
| tiny h2 | 48400 (44000-48400) | **56000** (56000-61600) | 26000 (26000-26000) |
| mix h1 | **26000** (23400-26000) | 24200 (22000-24200) | 20000 (20000-20000) |
| mix h2 | **26000** (23400-26000) | 24000 (24000-24000) | 16000 (16000-16000) |
| new TLS connection h1 | **14400** (14400-16000) | 14000 (14000-14000) | 12000 (12000-12000) |

The next table shows what each proxy used at that rate: CPU cores (of 6), peak PSS in MiB, and p99 in ms. It also shows the load host's k6 CPU in cores (of 12), which tells when the load generator, not the proxy, was the limit.

| workload | burrow | nginx | Caddy | k6 cores (burrow / nginx / Caddy) |
|---|---|---|---|---|
| tiny h1 | 2.1 / 80 / 6.5 | 1.9 / 71 / 4.7 | 3.4 / 95 / 8.0 | 4.7 / 5.8 / 3.5 |
| tiny h2 | 2.5 / 125 / 8.0 | 2.1 / 79 / 4.5 | 4.3 / 116 / 16.8 | 5.0 / 6.3 / 3.7 |
| mix h1 | 2.6 / 139 / 10.7 | 2.2 / 89 / 47.0 | 3.2 / 183 / 22.8 | 8.1 / 6.9 / 6.6 |
| mix h2 | 3.3 / 149 / 12.1 | 2.8 / 92 / 47.4 | 3.7 / 880 / 26.6 | 8.2 / 7.2 / 5.5 |
| new TLS connection h1 | 4.0 / 116 / 8.1 | 5.4 / 104 / 8.1 | 4.9 / 2129 / 17.1 | 10.7 / 9.7 / 8.4 |

How the steps above these rates failed:

- **burrow and nginx:** on every workload, a failed step had 0 errors and 0 wrong answers. It failed because more than 1 % of requests were dropped: answers got slower, and k6's 600 virtual users were all busy. Neither used all 6 cores: at tiny h1, burrow peaked at 2.6 cores and nginx at 2.2.
- **Caddy:**
  - tiny: it failed the same way, by dropped requests;
  - mix h1: it failed on errors (1-22 per step);
  - new connections: its p99 rose above 50 ms (73-109 ms);
  - mix h2: in 4 of its 6 failed steps (17600-32000/s), k6 itself ran out of its 4 GB. Answers slowed down and more requests stayed open at once in k6.
- **Load host near its limit:**
  - mix: k6 used 7-9 of 12 cores, and k6 straight to the app, with no proxy, held at least 16000 mix requests per second. So the mix results say "at least this much" for burrow and nginx.
  - new TLS connections: k6 used 10.6-11.6 of 12 cores with burrow. burrow held 16000/s in round 1, the highest rate the search tries. In rounds 2 and 3 the same 16000 failed on drops while k6 was at 11.5 of 12 cores. nginx used 5.4 of its 6 cores at 14000/s and failed at 15000/s.

### Same rate for all three

p50 ms / p99 ms / CPU cores / peak PSS MiB / errors, median of 3 rounds (the p99's lowest-highest in brackets).

| workload | rate/s | burrow | nginx | Caddy |
|---|---|---|---|---|
| tiny h1 | 15400 | 0.17 / 0.29 (0.29-0.31) / 1.06 / 37 / 0 | 0.15 / 0.28 (0.27-0.30) / **0.67** / 63 / 0 | 0.22 / 0.66 (0.65-0.68) / 2.23 / 79 / 0 |
| tiny h2 | 13000 | 0.19 / 0.30 (0.30-0.31) / 1.17 / 40 / 0 | 0.15 / 0.28 (0.27-0.28) / **0.59** / 65 / 0 | 0.26 / 1.82 (1.75-2.33) / 2.64 / 75 / 0 |
| mix h1 | 10000 | 0.20 / **2.60** (2.60-2.65) / 1.48 / **55** / 0 | 0.17 / 45.53 (45.53-45.77) / **1.05** / 65 / 0 | 0.25 / 3.23 (3.22-3.29) / 1.90 / 89 / 0 |
| mix h2 | 8000 | 0.22 / **2.80** (2.68-2.89) / 1.52 / **51** / 0 | 0.18 / 45.72 (45.62-45.81) / **0.96** / 67 / 0 | 0.30 / 2.89 (2.80-3.10) / 2.39 / 290 / 0 |
| new TLS connection h1 | 6000 | 0.23 / **0.52** (0.52-0.53) / **1.96** / 92 / 0 | 0.27 / 0.95 (0.93-1.02) / 2.61 / 75 / 0 | 0.29 / 0.78 (0.76-0.79) / 3.17 / 306 / 0 |

- **Small requests:** nginx uses the least CPU: 0.59-0.67 cores, against burrow's 1.06-1.17 and Caddy's 2.23-2.64. burrow's p99 is within 0.02 ms of nginx's.
- **Mix:** nginx's median is the lowest (0.17-0.18 ms), but its p99 is about 45 ms at every rate in this run, even at 500/s after the burst. burrow's and Caddy's p99 is 2.6-3.2 ms. This page does not look into nginx's cause; the configs above are the only settings.
- **New TLS connections:** burrow uses the least CPU per handshake: 1.96 cores at 6000/s, against 2.61 for nginx and 3.17 for Caddy. That holds even though burrow did the larger post-quantum handshake (see Setup). Its peak PSS here varied most between rounds: 58, 92 and 164 MiB.

### Burst, 2 minutes idle, light load

The burst offered 27000 requests/s: mix h1 at 18000/s plus mix h2 at 9000/s, both at once for 60 s. Median of 3 rounds (lowest-highest); memory is PSS in MiB.

| | burrow | nginx | Caddy |
|---|---|---|---|
| answered per second | 26830 (26825-26841) | 26771 (26744-26771) | 21613 (21540-21781) |
| errors | **0** (0-0) | 2 (0-3) | 127 (110-186) |
| wrong answers | 0 | 0 | 1 (1-2): status 431 to a normal request |
| dropped | 10216 (10024-10525) | 13760 (13719-15341) | 323099 (312987-327523) |
| p99 ms | **17** (17-18) | 52 (51-52) | 174 (174-179) |
| PSS before | 17 | 53 | 47 |
| PSS peak | 134 (133-137) | **94** (92-95) | 1008 (996-1037) |
| cgroup memory peak | 412 (408-439) | **268** (256-280) | 1102 (1091-1128) |
| PSS after 60 s idle | **34** (33-34) | 91 (90-92) | 1001 (996-1008) |
| PSS after 120 s idle | **34** (33-34) | 91 (90-92) | 1001 (996-1008) |
| light load (mix h2 500/s) p99 ms | 2.2 | 44.3 | 2.4 |
| PSS after the light load | **31** | 91 | 147 |

- **burrow and nginx kept up:** both answered about 26800/s, but burrow did it with 0 errors and a third of nginx's p99.
- **Caddy fell behind:** it answered 21600/s, made 110-186 errors, and gave 1-2 wrong answers per round, a `431 Request Header Fields Too Large` to a normal request.
- **Memory:** burrow's peak was above nginx's (134 against 94 MiB). Within 60 s of the end it was down to 34 MiB, below its own peak and below nginx (91 MiB). Caddy kept about 1 GiB through the whole idle time, and gave most of it back only once the light load started.

## Changes in v0.2.0

The same harness and machines ran twice on 2026-10-31: BENCH-1 tested `6dd548bf`, and BENCH-2 tested `4678505c`. These rows compare burrow's results then and now. The load host had 8 cores in BENCH-1 and 12 in BENCH-2 and this run, so the highest rates only compare between BENCH-2 and now. CPU, memory and errors compare across all three.

| | BENCH-1 (`6dd548bf`) | BENCH-2 (`4678505c`) | now (`e3028e08`) |
|---|---|---|---|
| tiny h1, highest rate | 28000 (errors from 30000) | 44000 held; 50000 failed on drops | 43200 (43200-48000) |
| tiny h2, highest rate | 28000 | 44000 held; 50000 failed on drops | 48400 (44000-48400) |
| new TLS connections, highest rate | 10000 | 13000 held (the highest step tried) | 14400 (14400-16000) |
| tiny h2 at 44000/s: CPU cores / peak PSS | n/a | 2.3 / 154 | 2.4-2.5 / 121-125 |
| mix h2 at 8000/s: p99 ms / CPU cores / peak PSS | 3.44 / 1.99 / 281 | 2.62 / 1.45 / 167 | 2.80 / 1.52 / **51** |
| burst: errors / PSS peak / cgroup peak / PSS idle | 111 / 1676 / 2109 / 36 (another burst) | 0 / 290 / 653 / 39 | 0 / **134** / **412** / 34 |

- **From BENCH-1 to BENCH-2:** burrow stopped refusing connections when many arrive at once, and kept less memory for the mix. A web connection without a handshake slot now waits for one, as in nginx and Caddy. The pool of handshake slots is larger (`limits.max_handshakes` 2048).
- **From BENCH-2 to now:** one pool of idle app connections is shared by all visitors, and memory went down:
  - mix at 8000/s: peak PSS 167 to 51 MiB;
  - burst: PSS peak 290 to 134 MiB, cgroup peak 653 to 412 MiB.

  The highest rates are in the same range as BENCH-2. CPU at the same rate is 5-11 % higher than in BENCH-2's single round: mix h2 at 8000/s went from 1.45 to 1.52 cores, and tiny h2 at 44000/s from 2.27 to 2.43-2.51. The mix p99 is 0.2 ms higher. BENCH-2 had one round, on another day, so this page claims no change in CPU or latency either way.

## Limits of this benchmark

- One machine for the proxies (6 cores), one for the load, one network. Other hardware, kernels and many visitor addresses can give different numbers.
- The load host is close to its own limit on mix and new connections: see the k6 cores column.
- Every request comes from one address (the load host). In burrow, the fair share per visitor only starts when half of a pool is in use, far above the 600 connections per k6 here.
- k6 gives each virtual user its own connection, so HTTP/2 here is not a browser sharing one connection between many requests.
- Only HTTP/1.1 and HTTP/2. HTTP/3, WebSocket and the tunnel to remote clients are not in this test: the app runs on the server (`client: self`).

## Reproduce

You need two Linux machines on one fast network, Docker on the target, and k6 v2.3.0 on the load machine.

1. Start the app, one proxy at a time, on the target with the configs above:
   - use the pinned images, or the burrow release binary in `alpine:3.22`;
   - `--network host --memory 4g --ulimit nofile=1048576`;
   - one self-signed ECDSA P-256 certificate for all three, made with `openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256`.
2. Point k6 at port 8443 with TLS 1.3 only and `constant-arrival-rate`, and check every answer as described under Workloads. Use `GODEBUG=http2client=0` for the HTTP/1.1 runs.
3. Sample each container's cgroup every second while k6 runs: `cpu.stat` (`usage_usec`), `memory.current`, and the PSS of its processes (`/proc/<pid>/smaps_rollup`).
4. Walk the rate up as in Workloads and method, three rounds, taking turns between the proxies.
