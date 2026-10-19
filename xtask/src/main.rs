#![forbid(unsafe_code)]
#![allow(clippy::disallowed_macros)]

mod arch;
mod size;
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
        Some("arch-check") => arch::run(&root),
        Some("tools") => tools::run(&root).map(|_| ()),
        Some("ui") => ui::main(&root, &args[1..]),
        Some("size") => size::run(&root, args.iter().any(|a| a == "--budget")),
        _ => Err(
            "usage: cargo xtask <arch-check | tools | ui [--release | --watch] | size [--budget]>"
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
