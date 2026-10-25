#![cfg(test)]

use burrow_admin_proto::codes::{PORT_IN_USE, WEBSOCKET_HOST, WEBSOCKET_MOVED, WEBSOCKET_OFF};
use burrow_admin_proto::msg::AdminLevel;
use burrow_admin_proto::ops::Op;
use burrow_admin_proto::plan::Plan;
use burrow_config::{ServerConfig, WebSocketConfig};
use burrow_syntax::grant::SELF_TRANSPORT;
use serde_json::json;

use super::*;

fn with_ws(extra: &str) -> World {
    let mut w = world();
    w.text = w.text.replace(
        "\nclients:\n",
        &format!("\n{extra}websocket: {{ listen: 7836 }}\n\nclients:\n"),
    );
    w
}

fn ws_path(key: Option<&str>) -> Vec<String> {
    let mut p = ServerConfig::WEBSOCKET.path();
    p.extend(key.map(str::to_string));
    p
}

fn set(path: Vec<String>, value: serde_json::Value) -> Vec<Op> {
    vec![Op::Set { path, value }]
}

fn compute(core: &Core, s: &Session, ops: &[Op]) -> Plan {
    let base = text::rev(&core.read().unwrap());
    match plan::compute(core, s, &base, ops) {
        Ok(c) => c.plan,
        Err(e) => panic!("{ops:?} was refused: {}", e.message),
    }
}

fn over_ws(w: &World) -> Session {
    Session::new(
        "ops".into(),
        AdminLevel::Write,
        "203.0.113.7:5000".parse().unwrap(),
        w.ops.clone(),
        true,
        "test".into(),
    )
}

fn home_over_ws() -> Vec<LiveSession> {
    vec![
        LiveSession {
            client: "home".into(),
            secret_fp: String::new(),
            from: "198.51.100.4:1".into(),
            over_websocket: true,
        },
        LiveSession {
            client: "office".into(),
            secret_fp: String::new(),
            from: "198.51.100.5:1".into(),
            over_websocket: false,
        },
    ]
}

fn impact<'a>(plan: &'a Plan, code: &str) -> Option<&'a str> {
    plan.impacts
        .iter()
        .find(|i| i.code == code)
        .map(|i| i.message.as_str())
}

#[test]
fn an_admin_over_websocket_cannot_move_or_turn_off_its_own_listener() {
    let w = with_ws("");
    let core = core_for(&w.text);
    let s = over_ws(&w);
    for ops in [
        set(ws_path(Some(WebSocketConfig::PATH.key())), json!("/other")),
        set(ws_path(Some(WebSocketConfig::LISTEN.key())), json!(7837)),
        vec![Op::Remove {
            path: ws_path(None),
        }],
    ] {
        let plan = compute(&core, &s, &ops);
        assert!(
            plan.refusals.iter().any(|r| r.code == SELF_TRANSPORT),
            "{ops:?}: {:?}",
            plan.refusals
        );
    }
    let plan = compute(
        &core,
        &s,
        &set(
            ws_path(Some(WebSocketConfig::KEEPALIVE_SECS.key())),
            json!(20),
        ),
    );
    assert!(plan.refusals.is_empty(), "{:?}", plan.refusals);
}

#[test]
fn moving_or_turning_off_websocket_names_the_clients_it_cuts_off() {
    let w = with_ws("");
    let core = core_of(&w.text, &w.text, Vec::new(), home_over_ws());
    let s = session(&w, AdminLevel::Write);
    let plan = compute(
        &core,
        &s,
        &set(ws_path(Some(WebSocketConfig::PATH.key())), json!("/other")),
    );
    assert!(plan.refusals.is_empty(), "{:?}", plan.refusals);
    let moved = impact(&plan, WEBSOCKET_MOVED).expect("a move warning");
    assert!(
        moved.contains("clients connected over WebSocket now (home) will be cut off and need their own config changed"),
        "{moved}"
    );
    let plan = compute(
        &core,
        &s,
        &[Op::Remove {
            path: ws_path(None),
        }],
    );
    let off = impact(&plan, WEBSOCKET_OFF).expect("an off warning");
    assert!(off.starts_with("turns WebSocket off; "), "{off}");
    assert!(off.contains("(home)") && !off.contains("office"), "{off}");
}

#[test]
fn turning_websocket_on_tests_its_port_first() {
    let w = world();
    let core = core_busy(&w.text, &w.text, vec![7836]);
    let s = session(&w, AdminLevel::Write);
    let plan = compute(&core, &s, &set(ws_path(None), json!({"listen": 7836})));
    let d = plan
        .diags
        .iter()
        .find(|d| d.code == PORT_IN_USE)
        .expect("a port-in-use error");
    assert_eq!(
        d.path,
        Some(ServerConfig::WEBSOCKET.then(WebSocketConfig::LISTEN).path())
    );
    let core = core_for(&w.text);
    let plan = compute(&core, &s, &set(ws_path(None), json!({"listen": 7836})));
    assert!(plan.valid, "{:?}", plan.diags);
}

#[test]
fn a_new_tunnel_name_names_the_clients_that_use_it() {
    let w = with_ws("");
    let text = w.text.replace(
        "websocket: { listen: 7836 }",
        "web: { https: 443, letsencrypt: { email: you@example.com } }\nwebsocket: { listen: 443, host: tunnel.example.com }",
    );
    let core = core_of(&text, &text, Vec::new(), home_over_ws());
    let s = session(&w, AdminLevel::Write);
    let plan = compute(
        &core,
        &s,
        &set(
            ws_path(Some(WebSocketConfig::HOST.key())),
            json!("t2.example.com"),
        ),
    );
    let named = impact(&plan, WEBSOCKET_HOST).expect("a name warning");
    assert!(
        named.contains("from tunnel.example.com to t2.example.com") && named.contains("(home)"),
        "{named}"
    );
}
