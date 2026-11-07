//! The admin UI logic that runs without a browser: changesets, checks, relations and rates.
//! It may depend only on burrow-admin-proto, burrow-config, burrow-syntax and semver.

#![forbid(unsafe_code)]

pub mod addr;
pub mod allow;
pub mod cert_words;
pub mod certs;
pub mod changeset;
pub mod check;
pub mod choices;
mod diag_words;
pub mod entity;
pub mod entries;
pub mod fallback;
pub mod health;
pub mod history;
pub mod input;
pub mod logins;
pub mod more;
pub mod off;
pub mod one_of;
pub mod own_update;
pub mod presence;
pub mod rates;
pub mod relations;
pub mod running;
pub mod settings_tabs;
pub mod shorthand;
pub mod top;
pub mod tree;
pub mod update_words;
pub mod version;
pub mod web;
pub mod wording;

#[cfg(test)]
mod testing;

pub use burrow_admin_proto::serde_json::Value;
