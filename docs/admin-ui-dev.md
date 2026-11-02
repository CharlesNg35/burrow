# Working on the burrow admin UI

How to build, run and test the admin UI while you change it. Using burrow admin: [admin.md](admin.md). The code rules are in [CONTRIBUTING.md](../CONTRIBUTING.md). Building and testing the rest of burrow: [development.md](development.md).

The UI is Rust compiled to WebAssembly (`crates/burrow-admin-ui`, Leptos), styled with Tailwind CSS v4 utility classes. No Node.js is needed.

One-time setup:

```bash
rustup target add wasm32-unknown-unknown
cargo xtask tools        # pinned wasm-bindgen, wasm-opt, the Tailwind standalone binary and Chrome for Testing, into .tools/
```

The loop, in three terminals:

```bash
cargo xtask ui --watch                                   # rebuilds the WASM and the CSS on every save
cargo run -- server -c dev/server.yaml                   # a local server with sample clients and services
cargo run -- admin -c dev/visitor.yaml --ui-dir crates/burrow-admin-ui/dist
```

`burrow admin` opens the browser; after a rebuild, refresh the page. `--ui-dir` serves the files from that folder instead of the embedded copy, and works only in debug builds. The configs in `dev/` hold public secrets: never use them on a real server.

After a UI change, run the browser flows:

```bash
cargo xtask e2e                       # every flow; or cargo xtask e2e e3 for the flows whose name matches e3
cargo xtask e2e --update-baselines    # records the visual baselines, in their own commit
```

`cargo xtask e2e` builds the release UI, then runs `tests/admin_e2e.rs` with the `admin-test-hooks` feature, against real burrow processes. It prints the path of the contact sheet: screenshots of every step, light and dark, in `tmp/admin-ui-shots/index.html` (`BURROW_E2E_SHOTS` changes the folder). A full run clears the old screenshots and writes its start time at the top of the sheet.

**Flows in parallel.** Each flow runs in its own process, with its own Chrome and burrow processes.

- `cargo xtask e2e` starts two flows, then one more every 3 seconds, up to 6 at a time. It adds one only while the one-minute load stays below 60 % of the cores, and the available memory stays above 6 GB plus 1.5 GB per running flow.
- It prints each decision, then the peak and the total time. Each flow's output is in `logs/` beside the contact sheet.
- `--jobs N` or `E2E_JOBS=N` (`make e2e E2E_JOBS=N`) sets the cap. `--jobs 1` runs one flow at a time, and so does `--update-baselines`, always.
- Where the load and memory cannot be read (outside Linux), it runs one flow at a time unless `--jobs` says otherwise.

**The pinned Chrome.** The flows use Chrome for Testing at the exact version in `tests/admin_e2e/baselines/chrome-version.txt`. `cargo xtask tools` downloads it into `.tools/` and checks it against the sha256 pinned in `xtask/src/tools.rs` (x86_64 Linux), so no setting is needed.

- `BURROW_E2E_CHROME` points at another Chrome, and `BURROW_E2E_SYSTEM_CHROME=1` uses the installed Google Chrome or Chromium.
- The visual baselines in `tests/admin_e2e/baselines/` are compared only when that Chrome has the same major version as `chrome-version.txt`. A different major fails with "run cargo xtask tools to get the pinned Chrome"; `BURROW_E2E_SKIP_BASELINES=1` skips the comparison on purpose.
- To move to a newer Chrome, change the version and sha256 in `xtask/src/tools.rs` and `chrome-version.txt` together, and re-record the baselines.

The browser flows do not run in CI. Before every tag, run `cargo xtask e2e` once on a machine with the pinned Chrome and keep the run green.

Other tests:

```bash
cargo test -p burrow-config -p burrow-admin-logic -p burrow-ui-kit -p burrow-admin-ui     # logic without a browser
cargo xtask ui --release && cargo xtask size --budget                                     # the release build and its size gate
```

On Linux and macOS, `make help` lists the same commands with the memory limits used here (`make e2e FLOW=e3`, `make test JOBS=8`).
