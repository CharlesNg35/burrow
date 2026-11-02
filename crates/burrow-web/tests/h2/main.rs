//! HTTP/2 tests of the proxy: basics, conformance, limits, connection reuse, stalls and HTTP/2 to apps.

mod basics;
#[path = "../common/mod.rs"]
mod common;
mod conformance;
mod early;
mod kit;
mod limits;
mod load;
mod reuse;
mod stall;
mod to_app;

pub use common::ws_echo;
