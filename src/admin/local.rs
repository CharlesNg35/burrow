//! The server machine's own `burrow clients`: the local status endpoint shows the admin's view of the clients and passes update requests to the admin core's single entry point.

use std::sync::Arc;

use burrow_admin_proto::msg::ErrorCode;
use burrow_admin_proto::updates::UpdateClients;

use super::Hub;
use super::core::Core;
use super::core::updates::{ON_THE_SERVER, ask_clients};
use crate::instance::publish::{Published, Snapshot, Update};

pub fn publish(config: &std::path::Path, hub: &Arc<Hub>) -> Option<Published> {
    crate::instance::publish::publish(config, snapshot(hub), Some(update(hub)))
}

fn snapshot(hub: &Arc<Hub>) -> Snapshot {
    let hub = hub.clone();
    Arc::new(move || hub.core.status())
}

fn update(hub: &Arc<Hub>) -> Update {
    let hub = hub.clone();
    Arc::new(move |body| answer(&hub.core, body))
}

fn failed(why: &str) -> String {
    serde_json::json!({ "error": why }).to_string()
}

fn code_of(code: ErrorCode) -> u16 {
    match code {
        ErrorCode::BadRequest => 400,
        ErrorCode::Forbidden => 403,
        ErrorCode::Conflict | ErrorCode::Invalid | ErrorCode::Busy => 409,
        _ => 500,
    }
}

pub fn answer(core: &Core, body: &[u8]) -> (u16, String) {
    let asked: UpdateClients = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return (400, failed(&format!("not an update request: {e}"))),
    };
    match ask_clients(core, &asked, ON_THE_SERVER) {
        Ok(reply) => (
            200,
            serde_json::to_string(&reply).expect("reply serializes"),
        ),
        Err(e) => (code_of(e.code), failed(&e.message)),
    }
}
