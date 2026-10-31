//! Project tasks for cargo xtask: architecture checks, the UI build, e2e flows, sizes, tools and the surface snapshot.
//! It is a developer tool; nothing in burrow or crates/ may depend on it.

#![forbid(unsafe_code)]
#![allow(clippy::disallowed_macros)]

mod arch;
mod compat;
mod e2e;
mod fields;
mod layers;
mod lints;
mod shape;
mod size;
mod surface;
mod tools;
mod ui;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn cargo() -> Command {
    Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
}

fn target_dir(root: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"))
}

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
        Some("compat") => compat::run(&root),
        Some("ui") => ui::main(&root, &args[1..]),
        Some("e2e") => e2e::main(&root, &args[1..]),
        Some("surface") => surface::main(&root, &args[1..]),
        Some("size") => size::run(&root, args.iter().any(|a| a == "--budget")),
        _ => Err(
            "usage: cargo xtask <arch-check [--edges] | compat | tools [--no-chrome] | ui [--release | --watch] | size [--budget] | surface [--out FILE] [--check FILE] | e2e [flow-filter] [--jobs N] [--update-baselines]>"
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
