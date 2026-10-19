# Working on the burrow admin UI

The UI is Rust compiled to WebAssembly (`crates/burrow-admin-ui`, Leptos), styled with Tailwind CSS v4 utility classes. No Node.js is needed.

One-time setup:

```bash
rustup target add wasm32-unknown-unknown
cargo xtask tools        # pinned wasm-bindgen, wasm-opt and the Tailwind standalone binary, into .tools/
```

The loop, in three terminals:

```bash
cargo xtask ui --watch                                   # rebuilds the WASM and the CSS on every save
cargo run -- server -c dev/server.yaml                   # a local server with sample clients and services
cargo run -- admin -c dev/visitor.yaml --ui-dir crates/burrow-admin-ui/dist
```

`burrow admin` opens the browser. After a rebuild, refresh the page. `--ui-dir` serves the files from that folder instead of the embedded copy and works only in debug builds. The configs in `dev/` hold public secrets: never use them on a real server.

Tests:

```bash
cargo test -p burrow-admin-logic -p burrow-ui-kit -p burrow-admin-ui     # logic without a browser
cargo xtask ui --release && cargo xtask size --budget                    # the release build and its size gate
cargo test --features admin-test-hooks --test admin_e2e -- --test-threads=1
```

The last line drives headless Google Chrome against real burrow processes. It skips with a message when Chrome is missing (`BURROW_E2E_CHROME` sets its path). Screenshots of every step and a contact sheet go to `tmp/admin-ui-shots/` (`BURROW_E2E_SHOTS` changes the folder). Visual baselines live in `tests/admin_e2e/baselines/`; update them on purpose, in their own commit, with `BURROW_E2E_UPDATE_BASELINES=1`.
