//! The command line, running a role and reloading its config, part of the ops layer.
//! It sits on top and may use every layer; nothing outside ops may depend on it.

pub mod cli;
pub mod command;
pub mod reload;
pub mod run;
