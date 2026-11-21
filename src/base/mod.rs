//! The base layer: generic helpers for errors, output, files, processes, prompts and ports.
//! It knows nothing about tunnels and imports no burrow module outside base.

pub(crate) mod access;
pub mod build;
pub mod channel;
pub mod failure;
pub mod fs;
pub mod logfile;
pub mod output;
pub(crate) mod ports;
pub mod process;
pub mod prompt;
pub mod redact;
pub mod slots;
pub mod style;
pub mod suggest;
#[cfg(all(test, feature = "web"))]
pub(crate) mod testkit;
#[cfg(windows)]
pub mod windows;
