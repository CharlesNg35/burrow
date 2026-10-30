use burrow_admin_proto::msg::{AdminLevel, ErrorCode, req};
use serde_json::json;

use super::{core_for, session, world};
use crate::admin::core::dispatch;

#[test]
fn every_request_but_apply_and_subscribe_reaches_a_handler_in_dispatch() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    for t in req::ALL
        .iter()
        .filter(|t| ![req::APPLY, req::SUBSCRIBE].contains(t))
    {
        if let Err(e) = dispatch(&core, &s, t, &json!({})) {
            assert_ne!(e.code, ErrorCode::UnknownRequest, "{t}: {}", e.message);
        }
    }
}
