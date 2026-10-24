#![cfg(feature = "web")]

#[path = "web/by_path.rs"]
mod by_path;
#[path = "web/certs.rs"]
mod certs;
mod common;
#[path = "web/ends.rs"]
mod ends;
#[path = "web/kit.rs"]
mod kit;
#[path = "web/passthrough.rs"]
mod passthrough;
#[path = "web/plain.rs"]
mod plain;
#[path = "web/plain_tunnel.rs"]
mod plain_tunnel;
#[path = "web/refusals.rs"]
mod refusals;
#[path = "web/reload.rs"]
mod reload;
#[path = "web/sites.rs"]
mod sites;
#[path = "web/tunnel.rs"]
mod tunnel;
