//! The admin UI logic that runs without a browser: changesets, checks, relations and rates.
//! It may depend only on burrow-admin-proto, burrow-config, burrow-syntax and semver.

#![forbid(unsafe_code)]

pub mod addr;
pub mod certs;
pub mod changeset;
pub mod check;
pub mod fallback;
pub mod grant;
pub mod input;
pub mod more;
pub mod rates;
pub mod relations;
pub mod tree;
pub mod version;
pub mod web;
pub mod wording;

#[cfg(test)]
mod testing;

pub use burrow_admin_proto::serde_json::Value;
