//! The config layer: finds, loads, checks, edits and saves burrow config files.
//! It may use base and wire; it must not depend on roles or ops.

pub mod admin_schema;
pub mod check;
pub mod diagnostics;
pub mod discover;
pub mod emit;
pub mod env;
pub mod grants;
pub mod load;
pub mod patch;
pub(crate) mod save;
pub mod schema;
pub mod settings;
pub mod tree;
pub mod validate;
pub mod web_summary;

pub use load::{Format, Loaded, Outcome, Role, load_file, load_str};
