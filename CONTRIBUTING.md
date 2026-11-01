# Contributing to burrow

These rules apply to all code in this repository, now and in the future. `cargo xtask arch-check`, clippy and CI check most of them.

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

- No code comments: no `//` and no `///`.
- One exception: a `//!` header of at most 3 lines, as the very first lines of each crate's `lib.rs` or `main.rs` (`burrow`, every crate in `crates/`, and `xtask`) and of each layer folder's `mod.rs` (`base`, `wire`, `policy`, `app`). Each of these files has one. It says what the unit is for and what it must not depend on.
- `cargo xtask arch-check` enforces the comment rule.
- Longer explanations belong in `docs/`, and the reasons for design choices in `docs/decisions.md`.

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
