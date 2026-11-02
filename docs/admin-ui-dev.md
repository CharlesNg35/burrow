# Working on the burrow admin UI

How to build, run and test the admin UI while you change it. Using burrow admin: [admin.md](admin.md). The code rules are in [CONTRIBUTING.md](../CONTRIBUTING.md). Building and testing the rest of burrow: [development.md](development.md).

The UI is Rust compiled to WebAssembly (`crates/burrow-admin-ui`, Leptos), styled with Tailwind CSS v4 utility classes. No Node.js is needed.

## How the admin UI is built

The UI never hard-codes the config. The server sends its **schema** (built by `#[derive(AdminSchema)]` from the model in `crates/burrow-config/src/model/`), its **config** and its **status**; the UI renders forms from the schema, keeps the user's edits in a draft, and asks the server to check and apply them. The server's check is the only judge: the UI shows its answers next to the fields.

| where | what it holds |
| --- | --- |
| `crates/burrow-config/src/model/` | the config model: every key, its label, help, units, examples, groups and relations (`#[admin(...)]`), and typed field paths such as `ServiceEntry::TARGET` |
| `crates/burrow-admin-logic` | the logic without a browser: checks that mirror the server's (`check`), choice lists for pickers (`choices`), relations (`relations`), "More settings" (`more`), wording (`wording`), the changeset (`changeset`) |
| `crates/burrow-ui-kit` | generic components: buttons, link buttons, inputs, pickers (`combo`), dialogs, tabs and scroll rows, charts, path diagrams |
| `crates/burrow-admin-ui/src/store.rs`, `api.rs`, `route.rs` | the one store (schema, config, status, the draft), the connection to `burrow admin`, the routes |
| `edit/` | the draft: changes, local errors, validation by the server after each pause, the plan to apply |
| `domain/` | one field: `domain::editor(FieldCtx)` picks the control for a schema node by its semantic type (text, number, duration, rate, list, map, address, reference) |
| `addr/` | address, target and client-entry controls, and the client and service pickers (`addr/picker.rs`) |
| `form/` | a whole object: cards and sections, "More settings", read-only view mode, "Reached by", groups of fields that do not apply |
| `screens/` | the pages: overview, the services, clients and visitors lists and pages (`entity/`), create wizards (`create/`), paths and flow diagrams, settings, activity; `screens/placement.rs` puts each field on a tab |
| `review/` | the Review page and its guards |
| `app/` | the shell: header, search, menus, popovers |
| `format.rs` | how times, sizes and counts read |

**Which tests guard what.**

- `screens/placement.rs`: every field of the live schema lands on exactly one tab, every server field has help, and fields whose format is not obvious show an example.
- `src/admin/core/agree.rs` (in the `burrow` crate): the UI's checks and the server's check agree on every value of every semantic type.
- `src/config/check/tests_codes.rs`: every diagnostic code the UI reads is one validate emits.
- `crates/burrow-admin-logic/src/diag_words.rs`: every diagnostic reads as plain words.
- `tests/admin_e2e/`: the browser flows, with screenshots and visual baselines.

### Recipes

**Add a config key.** Add the field to the model in `crates/burrow-config/src/model/` with its `#[admin(...)]` label, help (with an example when the format is not obvious) and, if it does not always apply, its relation. Add the server's check in `src/config/check/` and the runtime setting; the code that turns the model into settings destructures the struct, so the compiler shows where. The UI needs nothing more: the field appears on its entity's tab through `domain::editor`. `placement.rs` and `agree.rs` tell you if the help, the tab or a check is missing. Add the key to the docs; their YAML blocks are tested.

**Add a picker.** Never build a list of options in a screen. Get the choices from `burrow_admin_logic::choices` (extend it there, with a unit test, if it lacks a case), and show them through the shared picker in `addr/picker.rs` or `burrow_ui_kit::Combo`. Whether a client is online, is the server itself, or where it connects from comes from the one presence module, not from a lookup in the screen.

**Add a screen.** Add a variant to `Route` in `route.rs` (with its `parse` and `path`), a module in `screens/`, and its arm in `screens::screen`; add it to the header in `app/header.rs` if it is a top-level page. Build it from `burrow-ui-kit` components and the shared `form/` and `domain/` controls; a schema field on the screen goes through `domain::editor`, never a hand-made input. Add a browser flow in `tests/admin_e2e/` that opens it, and record its baselines.

## Build and run

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

The size gate allows 2 MiB of gzipped WebAssembly and 2.4 MiB for all UI assets with the fonts, and warns above 90 % of either (owner, 2026-11-02). The room is for necessary improvements, not a reason to add code.

On Linux and macOS, `make help` lists the same commands with the memory limits used here (`make e2e FLOW=e3`, `make test JOBS=8`).
