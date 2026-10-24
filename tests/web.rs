#![cfg(feature = "web")]

mod common;
#[path = "web/ends.rs"]
mod ends;
#[path = "web/kit.rs"]
mod kit;
#[path = "web/passthrough.rs"]
mod passthrough;
#[path = "web/plain.rs"]
mod plain;
#[path = "web/reload.rs"]
mod reload;
#[path = "web/sites.rs"]
mod sites;
#[path = "web/tunnel.rs"]
mod tunnel;
