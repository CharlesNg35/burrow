# Developing burrow

How to build burrow, run its tests, fuzz it and measure it. The code rules are in [CONTRIBUTING.md](../CONTRIBUTING.md), and working on the admin UI has its own page: [admin-ui-dev.md](admin-ui-dev.md).

- [What you need](#what-you-need)
- [Build](#build)
- [Cross builds](#cross-builds)
- [Tests](#tests)
- [Tests that need more tools](#tests-that-need-more-tools)
- [Heavy, soak and chaos tests](#heavy-soak-and-chaos-tests)
- [Lint and checks](#lint-and-checks)
- [Fuzzing](#fuzzing)
- [cargo xtask](#cargo-xtask)
- [make](#make)
- [Benchmarks](#benchmarks)
- [CI](#ci)

## What you need

- **Rust** from [rustup](https://rustup.rs/). `rust-toolchain.toml` pins the version (1.99.0), and rustup installs it on the first `cargo` command.
- **Nothing else** for the build and the main tests. The suites that need Docker, a browser or other tools say so below.
- On Linux and macOS, `make help` lists the common commands ([make](#make)). On Windows, use cargo and `cargo xtask` directly.

## Build

```sh
cargo build                                    # debug build: target/debug/burrow
cargo build --release                          # release build: target/release/burrow
cargo build -p burrow --no-default-features    # without the web proxy
```

- **The admin UI is built into the binary.** The build takes it from `crates/burrow-admin-ui/dist`. Without it, the binary carries a placeholder page instead. Build the UI first:

  ```sh
  rustup target add wasm32-unknown-unknown
  cargo xtask tools          # pinned wasm-bindgen, wasm-opt, Tailwind and Chrome for Testing, into .tools/
  cargo xtask ui --release   # the UI, into crates/burrow-admin-ui/dist
  cargo build --release
  ./target/release/burrow admin --ui-info   # what the binary carries
  ```

- `BURROW_ADMIN_UI_DIST=FOLDER` takes the UI from another folder, and `BURROW_ADMIN_UI_REQUIRE=1` makes the build fail when there is no UI.
- jemalloc's options are set at build time in `.cargo/config.toml` ([D53](decisions.md#d53-jemalloc-runs-its-background-purge-threads-so-an-idle-server-gives-memory-back), [D68](decisions.md#d68-one-in-memory-pipe-holds-a-32-kib-buffer-only-while-data-waits-and-jemalloc-returns-freed-pages-after-1-s)).

## Cross builds

The release workflow (`.github/workflows/release.yml`) builds these:

| release file | target | built with |
| --- | --- | --- |
| linux-amd64 | `x86_64-unknown-linux-musl` | cargo-zigbuild |
| linux-arm64 | `aarch64-unknown-linux-musl` | cargo-zigbuild |
| linux-armv7 | `armv7-unknown-linux-musleabihf` | cargo-zigbuild |
| linux-armv6 | `arm-unknown-linux-musleabihf` | cargo-zigbuild |
| linux-riscv64 | `riscv64gc-unknown-linux-musl` | cargo-zigbuild |
| freebsd-amd64 | `x86_64-unknown-freebsd` | cargo-zigbuild |
| windows-amd64 | `x86_64-pc-windows-gnu` | cargo-zigbuild |
| windows-arm64 | `aarch64-pc-windows-gnullvm` | cargo-zigbuild |
| macos-arm64 | `aarch64-apple-darwin` | cargo, on a Mac |
| macos-amd64 | `x86_64-apple-darwin` | cargo, on a Mac |

From Linux, every target except macOS builds with [zig](https://ziglang.org/) as the linker. The workflows pin zig 0.17.0 and cargo-zigbuild 0.23.4:

```sh
cargo install cargo-zigbuild --version 0.23.4 --locked   # and zig 0.17.0 on your PATH
rustup target add aarch64-unknown-linux-musl
cargo zigbuild --locked --release --target aarch64-unknown-linux-musl
```

- The binary is `target/<target>/release/burrow` (`burrow.exe` on Windows).
- For `aarch64-unknown-linux-musl`, the release sets `AARCH64_UNKNOWN_LINUX_MUSL_JEMALLOC_SYS_WITH_LG_PAGE=16`, so jemalloc also works on kernels with 64 KiB pages.
- On a Mac: `cargo build --locked --release --target aarch64-apple-darwin`.
- The release builds the admin UI once and gives it to every build with `BURROW_ADMIN_UI_DIST`.

## Tests

```sh
cargo test                                     # the burrow crate: unit tests and every suite in tests/
cargo test -p burrow-web -p burrow-io          # the web proxy, the lingering close and the in-memory pipe
cargo test -p burrow --no-default-features --lib   # the build without the web proxy
cargo test -p burrow-config -p burrow-admin-logic -p burrow-admin-proto -p burrow-syntax -p burrow-ui-kit -p burrow-admin-ui   # the admin UI's logic, without a browser
cargo test -p xtask                            # the checks of cargo xtask
cargo test -p burrow --lib tests_docs          # the YAML examples of docs/web.md, docs/web/ and docs/websocket.md are valid configs
```

**Over WebSocket.** `BURROW_TEST_TRANSPORT=ws` runs the tunnel suites with clients and visitors on WebSocket instead of TCP. CI runs this list:

```sh
BURROW_TEST_TRANSPORT=ws cargo test --test integration --test reconnect --test visit --test ranges --test failover --test socks --test proxy_protocol --test sources --test admin --test admin_keepalive --test self_target --test fetch --test web --test security
```

**As a system service.** These install burrow as a real service, run it and remove it again. They need administrator rights (`sudo` on Linux and macOS):

```sh
BURROW_E2E_PROFILE=dev bash tests/e2e/service.sh   # systemd on Linux, launchd on macOS
./tests/e2e/service.ps1                            # Windows, in PowerShell as administrator
```

Without `BURROW_E2E_PROFILE=dev`, `service.sh` makes a release build first.

**The `admin-test-hooks` feature.** It lets tests set what a server reports, such as an older version or a failing certificate, through `BURROW_ADMIN_TEST_*` variables. In the web proxy it lets the Let's Encrypt tests count refused nonces, send DNS checks to the test nameservers and shorten the wait between retries. It is for tests only: a release build with it does not compile. The admin UI flows, the Pebble tests and the DNS tests need it, and `cargo xtask e2e` turns it on for you.

## Tests that need more tools

Each of these tests returns at once and passes unless its variable is set, so `cargo test` stays quick. CI runs them with the pinned versions named here.

| what | command | needs |
| --- | --- | --- |
| certificates from Let's Encrypt's test server | `BURROW_PEBBLE=1 cargo test -p burrow-web --features admin-test-hooks --test acme -- --nocapture` | Docker: Pebble and pebble-challtestsrv 2.10.1 |
| DNS checks: BIND, acme-dns and a mocked Cloudflare API | `BURROW_PEBBLE=1 cargo test -p burrow-web --features admin-test-hooks --test dns01 -- --nocapture` | Docker: Pebble, BIND 9.20 and acme-dns 2.0.2 |
| HTTP/2 conformance | `BURROW_H2SPEC=/path/to/h2spec cargo test -p burrow-web --test h2 conformance -- --nocapture` | [h2spec](https://github.com/summerwind/h2spec) 2.6.0 |
| the stalled-stream watchdog and memory per HTTP/2 connection under load | `BURROW_H2_SLOW=1 cargo test -p burrow-web --test h2 -- stall:: load:: --nocapture` | nothing more; slow |
| gRPC through burrow to the grpc-go interop server | `BURROW_GRPC=1 cargo test -p burrow-web --test grpc -- --nocapture` | Docker: `golang:1.25-alpine`, which builds grpc-go 1.84.0's interop server |
| `wss://` through real nginx and Caddy | `tests/e2e/proxies.sh` | Docker |
| tunnels, `burrow mcp` and the admin UI against the released v0.1.2 | `cargo xtask compat` | `cargo xtask tools` and `cargo xtask ui` first; it downloads v0.1.2 and checks its sha256 |
| the admin UI flows in a real browser | `cargo xtask e2e` | `cargo xtask tools` (the pinned Chrome for Testing); see [admin-ui-dev.md](admin-ui-dev.md) |

The tests start the containers themselves, from images pinned by digest in the test code; `.github/workflows/ci.yml` pulls the same ones first.

## Heavy, soak and chaos tests

`tests/heavy.rs` holds the long tests, marked `#[ignore]`: 10,000 connections at once, a soak, chaos with dropped connections and restarts, reconnect cycles, and churn tests that check memory, open files and internal state go back to where they started. Run them on a release build, one at a time, with a high open file limit:

```sh
ulimit -n "$(ulimit -Hn)"
cargo test --release --test heavy -- --ignored --nocapture --test-threads 1
```

| variable | default | what it sets |
| --- | --- | --- |
| `BURROW_HEAVY_CONNS` | `10000` | connections at once in the scale test |
| `BURROW_SOAK_SECS` | `60` | how long the soak runs |
| `BURROW_SOAK_WORKERS` | `32` | connections the soak keeps busy |
| `BURROW_CHAOS_SECS` | `30` | how long the chaos test runs |
| `BURROW_CYCLES` | `300` | reconnect cycles |
| `BURROW_CHURN_ROUNDS`, `BURROW_CHURN_CONNS` | `20`, `2000` | rounds and connections per round of the churn tests |
| `BURROW_TEST_TRANSPORT` | `tcp` | `ws` runs them over WebSocket |

On GitHub, the `heavy` workflow runs them by hand (Actions, heavy, Run workflow). Its inputs are the soak time (1800 s), the connections (10000), the chaos time (120 s), the transport (`tcp` or `ws`) and which jobs to run: `heavy`, `proxies` (`tests/e2e/proxies.sh`), `freebsd` (the test suites in a FreeBSD 14 VM) or `all`.

## Lint and checks

CI fails on any of these:

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo clippy --locked --all-targets --features admin-test-hooks -- -D warnings
cargo clippy --locked -p burrow-web --all-targets --features admin-test-hooks -- -D warnings
cargo clippy --locked -p burrow --all-targets --no-default-features -- -D warnings
cargo clippy --locked -p burrow-admin-ui -p burrow-ui-kit -p burrow-config --target wasm32-unknown-unknown -- -D warnings
cargo xtask arch-check
cargo audit
cargo deny check advisories bans licenses sources
```

- `cargo xtask arch-check` checks the file sizes, the comment rule and the layers of [CONTRIBUTING.md](../CONTRIBUTING.md).
- `cargo audit` and `cargo deny` need `cargo install cargo-audit cargo-deny`.
- To lint the Windows build from Linux: `cargo-zigbuild clippy -p burrow --target x86_64-pc-windows-gnu --all-targets -- -D warnings`.
- The fuzz targets are linted on the stable toolchain: `(cd fuzz && RUSTUP_TOOLCHAIN=1.99.0 cargo clippy --all-targets -- -D warnings)`.

## Fuzzing

The fuzz targets live in `fuzz/`, a workspace of its own, and use [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) (`cargo install cargo-fuzz`). Run them from that folder:

```sh
cd fuzz
cargo fuzz list                                        # every target
cargo fuzz run config                                  # fuzz one target until you stop it
cargo fuzz run config -- -max_total_time=30 -max_len=4096   # what CI runs for each target
```

- cargo-fuzz needs a nightly Rust. `fuzz/rust-toolchain.toml` pins a dated nightly, and rustup uses it inside `fuzz/`; CI reads the same file.
- The nightly stays before 2026-11-01, because later nightlies reject burrow-admin-proto's recursive `SemType` with E0391, a rustc regression ([rust-lang/rust#165263](https://github.com/rust-lang/rust/issues/165263)), not a real cycle. Move it forward once that issue is closed.
- A crash is saved in `fuzz/artifacts/<target>/`. Replay it with `cargo fuzz run <target> <file>`, and turn it into a regular test before you fix it. Corpus and crash files are not committed.
- `make fuzz` runs a list of targets for a set time: `make fuzz FUZZ_TARGETS="config frames" FUZZ_SECS=60`.

## cargo xtask

Project tasks, written in Rust in `xtask/`, so they run the same on every system:

| command | what it does |
| --- | --- |
| `cargo xtask arch-check` | checks file and function sizes, comments and the layers; `--edges` prints the imports between units |
| `cargo xtask tools` | downloads the pinned wasm-bindgen, wasm-opt, Tailwind CSS and Chrome for Testing into `.tools/`, checked by sha256; `--no-chrome` leaves out Chrome |
| `cargo xtask ui` | builds the admin UI into `crates/burrow-admin-ui/dist`; `--release` for the small build, `--watch` to rebuild on every save |
| `cargo xtask size` | prints the gzipped size of the built admin UI; `--budget` fails above 1 MiB of WebAssembly or 1.2 MiB in all |
| `cargo xtask surface` | writes what burrow shows to its users and contributors to `target/surface.txt`: sizes, every `--help`, the schemas, the completions, the fuzz targets, the test names and the public items. `--out FILE` writes elsewhere, and `--check FILE` lists what changed against an earlier one, for example around a refactor |
| `cargo xtask e2e` | runs the admin UI flows in the pinned Chrome; a name filters the flows, `--jobs N` caps how many run at once, `--update-baselines` records the screenshots it compares ([admin-ui-dev.md](admin-ui-dev.md)) |
| `cargo xtask compat` | runs the tunnel, `burrow mcp` and admin UI tests against the released v0.1.2 binary |

## make

On Linux and macOS, the Makefile wraps the commands above with memory-friendly settings (4 build jobs, 4 test threads, line tables only for debug info):

```sh
make help                 # every command
make test JOBS=8          # change the build jobs, TEST_THREADS or DEBUG
make e2e FLOW=e3          # admin UI flows whose name matches e3
make ci                   # the CI lint and test jobs, locally
```

## Benchmarks

**The tunnel**, against rathole, frp and chisel ([benchmark.md](benchmark.md)). It runs on one Linux machine, over loopback, and needs `iperf3`, `python3` and `curl`:

```sh
bench/fetch.sh                                   # downloads the pinned rathole, frp and chisel releases and checks their sha256
bench/run.sh                                     # builds burrow, runs every setup, and writes docs/benchmark.md
TOOLS='burrow rathole' IDLE_COUNTS=1000 bench/run.sh   # a few setups only
```

- `bench/run.sh` overwrites `docs/benchmark.md`: commit it only from a full run on a quiet machine.
- `TOOLS`, `IDLE_COUNTS`, `IPERF_SECS` and `CONNRATE_SECS` pick the setups and the lengths; the downloads and results go to `~/.cache/burrow-bench` (`BENCH_DIR`).
- `cargo bench --bench cipher` measures the two ciphers alone.

**Websites**, against nginx and Caddy ([web-benchmark.md](web-benchmark.md)). It needs two Linux machines on one fast network: Docker on the one that runs the proxies and the app, and k6 on the other. The page gives the machines, the configs of all three proxies, the workloads, how each answer is checked, and the steps to run it again.

Compare numbers only between runs on the same machines, with nothing else running.

## CI

- **`ci`** runs on every push to `main` and every pull request: the lints above, the tests on Linux, macOS and Windows, each also as a system service, and on Linux over WebSocket, the Pebble and DNS tests, h2spec and gRPC, the admin UI build and its size, `cargo xtask compat`, 30 seconds of each fuzz target, the supply-chain checks, and the cross builds.
- **`heavy`** runs by hand: the [heavy tests](#heavy-soak-and-chaos-tests), the proxies and FreeBSD.
- **`release`** runs on a `v*.*.*` tag: every build above, the checksums, the Docker image and the GitHub release.
- The admin UI flows (`cargo xtask e2e`) do not run in CI. Run them before every tag ([admin-ui-dev.md](admin-ui-dev.md)).
