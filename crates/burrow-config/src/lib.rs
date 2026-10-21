#![forbid(unsafe_code)]

extern crate self as burrow_config;

pub mod admin_schema;
pub mod field;
pub mod model;
pub mod refs;

pub use field::{Entry, Field, Nest, entity_of_section};
pub use model::*;

pub const SELF: &str = "self";
