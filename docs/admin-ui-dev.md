# Working on the burrow admin UI

How to build, run and test the admin UI while you change it. Using burrow admin: [admin.md](admin.md). The code rules are in [CONTRIBUTING.md](../CONTRIBUTING.md). Building and testing the rest of burrow: [development.md](development.md).

The UI is Rust compiled to WebAssembly (`crates/burrow-admin-ui`, Leptos), styled with Tailwind CSS v4 utility classes. No Node.js is needed.

## How the admin UI is built

The UI never hard-codes the config. The server sends its **schema** (built by `#[derive(AdminSchema)]` from the model in `crates/burrow-config/src/model/`), its **config** and its **status**; the UI renders forms from the schema, keeps the user's edits in a draft, and asks the server to check and apply them. The server's check is the only judge: the UI shows its answers next to the fields.

| where | what it holds |
| --- | --- |
| `crates/burrow-config/src/model/` | the config model: every key, its label, help, units, examples, groups and relations (`#[admin(...)]`), and typed field paths such as `ServiceEntry::TARGET` |
| `crates/burrow-admin-logic` | the logic without a browser: checks that mirror the server's (`check`), choice lists for pickers (`choices`), relations (`relations`), "More settings" (`more`), wording (`wording`), the changeset (`changeset`), Go to's matching and order (`goto`) |
| `crates/burrow-ui-kit` | generic components: buttons, link buttons, inputs, pickers (`combo`), dialogs, tabs and scroll rows, charts, path diagrams, a part loaded from a lazy chunk (`Deferred`) |
| `crates/burrow-admin-ui/src/store.rs`, `api.rs`, `route.rs` | the one store (schema, config, status, the draft), the connection to `burrow admin`, the routes |
| `edit/` | the draft: changes, local errors, validation by the server after each pause, the plan to apply and its change groups; the text edit of the config files (`text.rs`) and its tabs (`tabs.rs`) |
| `entities.rs` | adding a client, visitor or service to the draft (`create_with`), and the name check of a new one |
| `format.rs` | how times, sizes and counts read, and a client's presence with this server's reserved names |
| `field.rs` | what every control gets: `FieldCtx`, `set_checked` (sets a value after the shared check), plain values and links to entries |
| `addr/` | address, target and client-entry controls, and the client and service pickers (`addr/picker.rs`) |
| `shown.rs`, `row_menu.rs` | the one popover behaviour (focus, Escape, click outside) and the ··· row menu |
| `domain/` | one field: `domain::editor(FieldCtx)` picks the control for a schema node by its semantic type (text, number, duration, rate, list, map, address, reference) |
| `form/` | a whole object: cards and sections, "More settings", read-only view mode, "Reached by", groups of fields that do not apply |
| `rename.rs` | the rename dialog and the name input of every create and rename |
| `screens/` | the pages: overview, the services, clients and visitors lists and pages (`entity/`), create wizards (`create/`), paths and flow diagrams, settings, activity, the Raw config tab with the text editor, Go to and Edit as (`config_file/`), lists grouped by file (`file_groups.rs`); `screens/placement.rs` puts each field on a tab |
| `review/` | the Review page and its guards |
| `app/` | the shell: header, search, menus |

These folders are in layers (model, controls, editor, pages; see [CONTRIBUTING.md](../CONTRIBUTING.md#layers)): a control never imports a page, and `cargo xtask arch-check` enforces it. Entity facts (name, icon, examples, wording) are in `burrow_admin_logic::entity`, a client's online state and address in `burrow_admin_logic::presence`, and the lists of choices in `burrow_admin_logic::choices`.

**Which tests guard what.**

- `screens/placement.rs`: every field of the live schema lands on exactly one tab, every server field has help, and fields whose format is not obvious show an example.
- `src/admin/core/agree.rs` (in the `burrow` crate): the UI's checks and the server's check agree on every value of every semantic type.
- `src/config/check/tests_codes.rs`: every diagnostic code the UI reads is one validate emits.
- `crates/burrow-admin-logic/src/diag_words.rs`: every diagnostic reads as plain words.
- `src/admin/core/tests/editor_schema/` (in the `burrow` crate): the text editor's JSON Schema marks a problem only where burrow's own check has an error (D89).
- `tests/admin_e2e/`: the browser flows, with screenshots and visual baselines.
- `tests/admin_e2e/design.js`: before every screenshot, the rules of [Messages, placement and layout](#messages-placement-and-layout); `xtask/src/glyphs.rs`: no symbol the fonts lack.

### Recipes

**Add a config key.**

1. Add the field to the model in `crates/burrow-config/src/model/` with its `#[admin(...)]` label, help (with an example when the format is not obvious) and, if it does not always apply, its relation.
2. Add the server's check in `src/config/check/` and the runtime setting; the code that turns the model into settings destructures the struct, so the compiler shows where.
3. The UI needs nothing more: the field appears on its entity's tab through `domain::editor`. `placement.rs` and `agree.rs` tell you if the help, the tab or a check is missing.
4. Add the key to the docs; their YAML blocks are tested.

**Add a picker.** Never build a list of options in a screen. Get the choices from `burrow_admin_logic::choices` (extend it there, with a unit test, if it lacks a case), and show them through the shared picker in `addr/picker.rs` or `burrow_ui_kit::Combo`. Whether a client is online, is the server itself, or where it connects from comes from `burrow_admin_logic::presence` (or `format::presence_of`, which adds this server's reserved names), never from a lookup in the screen; arch-check refuses `clients.iter().find(` elsewhere.

**Add a screen.**

1. Add a variant to `Route` in `route.rs` (with its `parse` and `path`), a module in `screens/`, and its arm in `screens::screen`; add it to the header in `app/header.rs` if it is a top-level page.
2. Build it from `burrow-ui-kit` components and the shared `form/` and `domain/` controls; a schema field on the screen goes through `domain::editor`, never a hand-made input.
3. Add a browser flow in `tests/admin_e2e/` that opens it, and record its baselines.

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

`RUSTFLAGS` or `CARGO_ENCODED_RUSTFLAGS` in the environment would make cargo skip the wasm flags in `.cargo/config.toml`, so `cargo xtask ui` passes the wasm build one `CARGO_ENCODED_RUSTFLAGS` with your flags first, then those config flags and the `-Clink-arg=--emit-relocs` the chunk split needs.

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

The size gate allows 2 MiB of gzipped WebAssembly for the main module, 3.5 MiB for all WebAssembly with the lazy chunks, and 0.4 MiB more for the fonts, CSS, JavaScript and HTML (3.9 MiB in all), and warns above 90 % of each. The room is for necessary improvements, not a reason to add code.

**Lazy chunks.** A large part that only some admins open (the raw config editor) can load on demand from its own WebAssembly chunk, so the first page stays small. Mark one function as the split point and show it through `burrow_ui_kit::Deferred`, which shows "Loading …" and, when the chunk cannot be fetched, a "Try again" button:

```rust
// crates/burrow-admin-ui/src/screens/example.rs
#[leptos::wasm_split::wasm_split(big_part, wasm_split_path = ::leptos::wasm_split, fallible)]
fn big_part(props: BigProps) -> Result<AnyView, burrow_ui_kit::LoadFailed> {
    Ok(view! { <BigPart props/> }.into_any())
}

view! { <Deferred load=move || big_part(props.clone()) what="the big part"/> }
```

- Everything only that function reaches moves into `split_big_part-<hash>.wasm`; `cargo xtask ui` splits the module with `wasm_split_cli_support` before wasm-bindgen and writes a small loader, `__wasm_split.<hash>.js`. `cargo xtask size` reports the main wasm and the chunks apart.
- Use `fallible`, not Leptos' `#[lazy]`: without it a failed fetch panics, and a panic stops the whole UI.
- The function takes its arguments by value and returns `Result<AnyView, LoadFailed>`; it is called again on "Try again".
- No wasm may use WebAssembly SIMD, so burrow admin keeps working in browsers without it (Safari before 16.4); `cargo xtask ui` fails naming the file that does. One crate with a `#[target_feature(enable = "simd128")]` function is enough to put SIMD into the whole module (D88).

On Linux and macOS, `make help` lists the same commands with the memory limits used here (`make e2e FLOW=e3`, `make test JOBS=8`).

## Messages, placement and layout

These rules say where a message goes, how long it may be and how a row lines up. They apply to every screen and dialog. Each rule says how it is checked:

- `e2e`: `tests/admin_e2e/design.js` runs before every screenshot of the browser flows (`Ui::shot`) and fails the step with the rule's number. It checks this UI only: a compat flow that drives an older burrow admin (`Options::admin_bin`) skips it, since that UI is fixed. `BURROW_E2E_SKIP_DESIGN=1` turns it off while you debug a flow.
- `arch-check`: `cargo xtask arch-check` (`xtask/src/glyphs.rs`) reads the UI crates' source.
- `review`: no automatic check; reviewers and the screenshots catch it.

The e2e check finds the header line by `data-header-line` (set by `PageHeader` and the Overview and Review headers), a card subline by `data-card-sub` (set by `card_head`) and a calm summary by `data-calm`.

### 1. One home per kind of message

| kind of message | where it lives | example |
|---|---|---|
| what the page is, in a few words | the page header line (`PageHeader` `sub`) | "29 services, 3 reached by websites" |
| a fact about one card | that card's subline (`card_head` `sub`) or a row inside it | "3 of 4 clients up" |
| something the user should look at or fix | Needs attention (Overview), and in place next to the field or row it is about | "The certificate for portal.example.com expires in 9 days" |
| the user must act before anything else works | a `Banner` under the header: connection lost, config errors, config changed on the server | "Connection lost. Retrying…" |
| the answer to what the user just did | one toast | "Added to changes" |
| what happened, with its full detail | Activity | the apply, its diff, the server's log line |
| why a control is off, what a short value means | a tooltip (`title` or `Tooltip`) | button "Update" disabled, tooltip "the client runs as a container" |
| technical detail a few users need | a "Details" fold (`Disclosure`) under the plain sentence | error code, path, checksum, raw server text |

Good: Apply succeeds, a toast says "Applied", the header keeps its usual line, Activity lists the version. Bad: the header line also says "Applied version 12 at 10:32", and a callout repeats it.

Check: e2e, a toast's text (normalised, at least 12 characters) is not part of a visible header line or card subline.

### 2. Lengths

| place | at most | on a phone (390 px) |
|---|---|---|
| page header line | 80 characters, one sentence | 2 lines |
| card subline | 50 characters | 1 line, else it moves under the title |
| toast | 80 characters, plus one action word | 3 lines |
| Needs attention item title | 70 characters; detail goes on its second line | 3 lines |
| button label | 3 words | 1 line, never broken inside |
| tooltip | 120 characters | n/a |

A message that does not fit is the wrong kind: move the detail to Details, Activity or a tooltip, do not shorten the words into jargon.

Good: header "3 clients offline". Bad: header "Could not reach the local status endpoint at 127.0.0.1:27801 (connection refused); showing the last known state from 10:31".

Check: e2e, the header line (characters, sentences, and lines below 500 px), the card subline and the toast, at every step and width the flows use.

### 3. One message per line, by priority

A line (header line, card subline, table cell, status row) says one thing. When several states are true, the screen shows the highest one and leaves the rest to their own homes. The order is: broken > in progress > needs a look > available > recently done > nothing. "Nothing" means no line at all, not "All good".

Good: "Updating to 0.2.3…" while an update runs. Bad: "Up to date · Update available · Checked 2m ago" on one line.

Check: e2e, a header line holds one sentence (no `. ` followed by a capital); review for the rest.

### 4. Problems go to Needs attention, and the page never contradicts itself

Every problem (a failing check, an offline client that serves something, an expiring certificate, a failed update) is an item in Needs attention and, where it can be fixed, also next to the thing it is about. A page never shows a calm summary next to a problem: no "Nothing needs attention", "All clients online" or a green dot while a danger or warning item is on the same page.

Good: a failing health check shows in Needs attention and as a red row in Health per client. Bad: "Nothing needs attention" in the card and "Update failed" in the header line.

Check: e2e, while a calm summary (`data-calm`, today "Nothing needs attention") is visible, no visible `[data-tone=danger]` or `[data-tone=warning]` exists outside toasts and dialogs.

### 5. Plain words first, technical detail behind Details

The first line is what happened and what to do, in the user's words. The About page is the one place for support facts (versions, schema, paths) and is exempt. Addresses, paths, codes, raw server text and internal names go behind a "Details" fold or into Activity. Never name an internal part (endpoint, API, schema path, store, draft, `web.certificates[0]`).

Good: "The server could not check this client" + Details: "GET /status: connection refused". Bad: "local status endpoint unreachable", "2 CIDRs", "(tls: internal)".

Check: e2e, visible text outside `code`, `pre`, `.font-mono`, inputs, the About page and `[data-details]` may not contain endpoint, payload, schema, struct or a key path with an index like `certificates[0]`.

### 6. Tone colours

- `Danger` only for a broken state now: a service nothing can reach, a failed check, a failed apply or update, a lost connection.
- `Warning` for "needs a look": expiring soon, an offline client, a setting that will cut someone off.
- `Success` only as the answer to an action or a positive summary with nothing else to say.
- `Info` (accent) for neutral notes. Never red for a choice the user made (off, disabled, not set).
- A danger card is never red-on-red: `DANGER_CARD`.

Good: an offline client is amber. Bad: a client the user turned off is red.

Check: review; the contrast tests in `crates/burrow-admin-ui/src/contrast.rs` keep each tone readable in light and dark.

### 7. No redundant text

Do not write what a visible control or icon already says: no "Update available" next to an "Update" button, no "Click to edit" next to Edit, no "Status: online" next to a green dot with the word online, no repeated entity name in a card that sits under that name. Helpful repeats across places are fine.

Good: a button "Update to 0.2.3". Bad: "0.2.3 is available" followed by a button "Update to 0.2.3".

Check: review.

### 8. Rows of text and buttons line up and wrap whole

- A row that mixes text, tags, icons and buttons is `flex items-center`; inline buttons use `ButtonSize::Small` so they do not stretch the row. The vertical centres of its items differ by at most 2 px. Next to text of several lines, a control lines up with the first line or with the middle of the text, as the row's alignment says.
- Table cells of one row line up on their first line of text, whatever their font (mono, tag or text). Tables use `burrow_ui_kit::Table`, not a hand-made `<table>`.
- On a narrow screen a row wraps whole items: a button, a tag or a "label: value" pair never breaks inside (`whitespace-nowrap` on the item, `flex-wrap` on the row).
- A separator ("·") belongs to the item after it and is never the first or the last thing on a visual line. Use `burrow_ui_kit::Inline`, which hides a separator that would start a line; never type " · " between items that may wrap.
- Buttons side by side have a gap.

Good: `Client 0.2.2 [Update]` with the button centred on the text. Bad: the button 6 px lower than the text, or a line on a phone that starts with "· 2 services".

Check: e2e, for every visible flex row, a control and a text item on the same line have centres within 2 px, measured on the first line of the text or on the whole text; the cells of a table row have the middles of their first lines (baseline minus half the x-height) within 2 px; every visible "·" has text on its own line on both sides. The ui-kit `Inline` component draws its separators in CSS and hides one that would start a line.

### 9. Icons come from the ui-kit

Use `Icon` (`burrow_ui_kit::icon`) for every pictogram: warning, external link, copy, close, check, arrows in buttons. Never type a glyph such as ⚠ ↗ ⧉ ✕ ✓ ↵ ⟳: the bundled Geist fonts are a Latin subset without them, so they render as empty boxes, and they do not follow the tone colours. A new pictogram is a new `Icon` name, not a character.

Characters the fonts have may be text: · … — – − × • › ↑ ↓. The arrows → and ← are not in Geist but every system font has them, so they stay allowed as text ("Open websites →"). Other symbols (≠ ≈ ↔) become words or an icon.

Good: `<Icon name="external"/>` after "Release notes". Bad: "Release notes ↗".

Check: arch-check refuses any other character in the product code of `burrow-admin-ui`, `burrow-ui-kit` and `burrow-admin-logic` (Latin-1, General Punctuation and the list above pass; use `\u{...}` only for invisible marks). e2e tests every visible character in the browser: a canvas measures it with `Geist, serif`, `Geist, monospace` and an unassigned code point; equal to the unassigned one is an empty box, different between the two fallbacks is another font.

### 10. Phones

- Check every screen at 390 px (and 360 px for new layouts), light and dark, and at 1440 and 1280.
- No page scrolls sideways; tab, chip and navigation rows use `ScrollRow`.
- Header actions move under the title as one group; the header line wraps to at most 2 lines.
- Touch targets are at least 24 × 24 px (WCAG 2.2 2.5.8).
- Tables become stacked rows, not a sideways-scrolling grid, unless they are data tables a user reads across.
- Text that is cut has an ellipsis and its full value on hover (`auto_titles`).

Good: the client page header on a phone: name, then the status dot and tags, then the buttons on one row. Bad: the Edit button pushed off-screen by a long header line.

Check: e2e, the header line on a phone (rule 2) and the layout checks of every flow at 390 px; review for the rest.

### 11. Toasts and the save bar leave room

While a toast or the save bar shows, the page leaves room below its last card, so every card can be scrolled fully above them. The room only grows while the same page stays open, so nothing jumps when a toast fades.

Check: e2e, `tests/admin_e2e/clear.js` runs with `design.js` before every screenshot: scrolled to the end, no visible card lies under a toast or the save bar.
