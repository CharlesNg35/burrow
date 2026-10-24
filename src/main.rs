//! The burrow binary: it hands the command line to the library's app layer.
//! It holds no logic of its own and must not depend on the admin UI crates.

#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    burrow::app::command::main()
}
