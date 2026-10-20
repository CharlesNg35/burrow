#![forbid(unsafe_code)]

extern crate self as burrow_config;

pub mod admin_schema;
pub mod model;

pub use model::*;
