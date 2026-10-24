//! Project tasks for cargo xtask: architecture checks, the UI build, e2e flows, sizes, tools and the surface snapshot.
//! It is a developer tool; nothing in burrow or crates/ may depend on it.

#![forbid(unsafe_code)]
#![allow(clippy::disallowed_macros)]

mod arch;
mod e2e;
mod fields;
mod layers;
mod size;
mod surface;
mod tools;
mod ui;

use std::path::PathBuf;
use std::process::ExitCode;

fn root() -> PathBuf {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    here.parent().map(PathBuf::from).unwrap_or(here)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = root();
    let result = match args.first().map(String::as_str) {
        Some("arch-check") if args.iter().any(|a| a == "--edges") => layers::print_edges(&root),
        Some("arch-check") => arch::run(&root),
        Some("tools") => tools::main(&root, &args[1..]),
        Some("ui") => ui::main(&root, &args[1..]),
        Some("e2e") => e2e::main(&root, &args[1..]),
        Some("surface") => surface::main(&root, &args[1..]),
        Some("size") => size::run(&root, args.iter().any(|a| a == "--budget")),
        _ => Err(
            "usage: cargo xtask <arch-check [--edges] | tools [--no-chrome] | ui [--release | --watch] | size [--budget] | surface [--out FILE] [--check FILE] | e2e [flow-filter] [--jobs N] [--update-baselines]>"
                .to_string(),
        ),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
