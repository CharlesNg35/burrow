# Contributing to burrow

These rules apply to all code in this repository, now and in the future. `cargo xtask arch-check`, clippy and CI check most of them.

To build and test burrow, see [docs/development.md](docs/development.md).

## Code shape

- A source file has at most 600 lines of code, not counting its inline test module. Aim for under 400.
- An inline test module has at most 300 lines. A longer one moves to a sibling `tests.rs`.
- A test file has at most 1000 lines. A longer one becomes a folder of modules. Test files are everything under a `tests/` folder, every `tests.rs`, and every file that starts with `#![cfg(test)]`.
- A function has at most 150 lines; clippy's `too_many_lines` denies longer ones in every crate. Aim for 100 or less in new code, and bring an older function under 100 when you change it.
- `cargo xtask arch-check` enforces the file limits, with no exceptions.
- Split by responsibility: one clear job per file, not many tiny files.
- Write new code within these limits from the start, instead of splitting it later.

## Layers

The `burrow` crate is built in layers, from the bottom up:

| Layer | What it holds |
|---|---|
| base | generic helpers: errors, output, files, processes, prompts, ports |
| wire | secrets, encryption, records, the protocol, transports |
| config | the config, its checks, policies, install records, metrics |
| roles | dialing, forwarding, UDP relays, the server, the client, the visitor |
| ops | services, instances, update, edit, init, status, admin, the CLI |

- A lower layer never imports a higher one, and no two units import each other. A unit is a module listed in `xtask/src/layers.rs`.
- Each crate in `crates/` has a fixed list of crates it may depend on.
- A new module gets its row in `xtask/src/layers.rs`, and a new crate in `xtask/src/arch.rs`, in the same change.

## Comments

- No code comments: no `//`, no `///` and no `/* */`.
- One exception: every module root starts with a `//!` header of at most 5 lines. A module root is a `lib.rs`, `main.rs` or `mod.rs`, or a `foo.rs` that has a `foo/` folder beside it; this includes test crates and the test kits under `tests/`.
  - Line 1 is one whole sentence that says what the module is for, and still reads right on its own.
  - The other lines are optional: where to start reading (the entry point or key type), what the module must not depend on, or a short reason for its shape.
  - Write it from the code, and change it when the module changes.
- Other files (leaf modules) have no header; their module root and their names say what they hold.
- `cargo xtask arch-check` enforces all of this.
- Longer explanations belong in `docs/`, and the reasons for design choices in `docs/decisions.md`. The map of the code is in [docs/development.md](docs/development.md#code-map).

## Changes

- Every behaviour fix starts with a test that fails without it.
- Never weaken or delete a test without saying why.
- Every commit compiles on its own.
- Commit messages are a single line that says what is true now, for example "A visitor's UDP forward forgets its peers after the server closes the visit". No trailers.
- Stage files by name; never `git add -A`.
- When `rust-toolchain.toml` moves to a newer Rust, check D52 in `docs/decisions.md` (64-bit time on the 32-bit ARM builds).
- The fuzz targets build on the dated nightly in `fuzz/rust-toolchain.toml`, which CI reads too. It stays before 2026-11-01 because later nightlies reject burrow-admin-proto's recursive `SemType` with E0391, a rustc regression (rust-lang/rust#165263), not a real cycle; move it forward once that issue is closed.

## Design

- No backward compatibility shims or migrations, unless a release says otherwise.
- New features are off unless they are written in the config, and `burrow validate` shows what will happen.
- Use a well-tested library instead of writing your own. For a large dependency, measure the size it adds first.
- Every implementation follows current best practice, not older patterns. Before you build on a protocol, an OS behaviour, a library API or a security choice, confirm the current recommendation in the official docs (RFCs and their errata, crate docs, man pages, vendor docs) with a web search. Do the same whenever you are unsure, and cite the source in `docs/decisions.md` or the commit notes. Never guess.
- A simplification must not remove a feature, useful UI information or a documentation example.

## One source for each fact

burrow has one config model shared by the server, the CLI and the admin UI. Most bugs that slip through review come from a second copy of something: a second list of choices, a second check, a second label. These rules keep each fact in one place.

- **One trait per concept, one file per implementation.** A family of interchangeable things (login kinds, DNS providers, transports, notification targets) has a small trait for the part that varies. Each implementation lives in its own file with its own config, format and tests, and adding one is a new file plus one registry line.
- **No bool mode flags.** A look, a size or a mode is an enum, never a `bool` or a string, so a typo or a wrong order does not compile.
- **Shared logic once.** Queues, retries, parsing, checks and formatting that two places need live in one function, in the lowest crate both may use (`burrow-syntax`, `burrow-config`, `burrow-admin-proto`, `burrow-admin-logic`). The server, the CLI and the admin UI call it.
- **One component per concept.** The admin UI has one component for each kind of control (a button, a link button, a picker, a status dot, a row of tabs), in `burrow-ui-kit` or the shared UI folders. A page never styles its own copy; if the component lacks something, extend it.
- **One source per list of choices.** Every picker of clients, visitors, services or other named things gets its options from the shared choices builder (`burrow_admin_logic::choices`). A screen never builds its own list, even a simple one.
- **One place per derived fact.** Whether a client is online, whether it is the server itself, where it connects from, and how durations, sizes and status words read: each has one function. Use it; do not look a status up by name in a screen.
- **Schema fields render through the field editor.** A field of the config model is shown and edited through the shared editor (`domain::editor`), also in wizards. Its label, help, units and checks come from the schema, never from a copy.
- **Validate with the shared rules.** UI checks call the same `burrow-syntax` and `burrow-admin-logic` functions the server uses. No inline `contains('@')` or `parse().ok()`.
- **A relation lives in the model.** If a field does not apply in some case, the model says so (for example with `enabled_if`), and the server check and the UI both read that one rule; never a second hand-written list.
- **New model fields are checked by the compiler all the way to the runtime.** Code that turns the model into runtime settings destructures the model struct without `..`, so a new field cannot be forgotten.
- **Test helpers are shared.** Before you write a helper in a test file, look in the test kits (`tests/common`, `src/base/testkit.rs`, the crate's own kit). A second copy moves into the kit.
- **When you add a variant, find every list of variants.** Add or extend a test that walks all of them (schema fields, entity kinds, choice lists, diagnostic codes), so the next person cannot miss one.
