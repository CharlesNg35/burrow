//! The base layer: generic helpers for errors, output, files, processes, prompts and ports.
//! It knows nothing about tunnels and must not import any other burrow module.

pub(crate) mod access;
pub mod failure;
pub mod fs;
pub mod logfile;
pub(crate) mod ports;
pub mod process;
pub mod prompt;
pub mod style;
pub mod suggest;
