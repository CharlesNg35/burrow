//! The burrow config model: the server, client and visitor files, their typed field paths and their admin schema.
//! It must not depend on burrow, tokio or the UI crates.

#![forbid(unsafe_code)]

extern crate self as burrow_config;

pub mod admin_schema;
pub mod field;
pub mod model;
pub mod refs;

pub use field::{Entry, Field, Nest, entity_at, entity_of_section, entity_path, logins_path};
pub use model::*;

pub const SELF: &str = "self";
