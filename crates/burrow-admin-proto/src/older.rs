#![cfg(test)]

use serde_json::json;

use crate::msg::{AuditEntry, HistoryReply, HistoryRequest, State, Welcome};
use crate::plan::{Applied, Plan};
use crate::status::Status;

#[test]
fn what_a_v0_1_2_server_sends_still_decodes() {
    let w: Welcome = serde_json::from_value(json!({
        "server": {"version": "0.1.2", "target": "x86_64-unknown-linux-musl", "uptime_secs": 5, "name_hint": ""},
        "protocol": 1,
        "you": {"visitor": "ops", "level": "write", "from": "127.0.0.1", "secret_fp": "ab12"},
        "features": ["status.stream", "config.events", "secret.generate", "connect.snippets", "suggest.free-port", "history", "revert", "ops.rename", "ops.set-text"],
        "limits": {"max_message": 4194304, "max_inflight": 8},
        "schema_hash": "b2s:1"
    }))
    .unwrap();
    assert!(!w.has(crate::features::HISTORY_IDS));
    assert!(!w.has(crate::features::CLIENT_ALLOW));
    let s: State = serde_json::from_value(json!({
        "rev": "b2s:1",
        "file": {"path": "/etc/burrow/server.yaml", "format": "yaml", "writable": true},
        "running": {"rev": "b2s:1", "ok": true, "file_diags": []},
        "values": {"services": {}},
        "runtime": {"sessions": {}, "addresses": [], "bound": {}, "restart_pending": [], "allows": {}},
        "diags": [{"severity": "warning", "code": "x", "message": "m", "path": ["a"]}]
    }))
    .unwrap();
    assert_eq!(s.rev, "b2s:1");
    let st: Status = serde_json::from_value(json!({
        "version": "0.1.2", "uptime_secs": 1, "listening": ["0.0.0.0:7835"],
        "handshakes_refused": 0, "udp_peers": 0,
        "clients": [{"name": "home", "connected": true, "connected_secs": 3, "from": "1.2.3.4", "open": 0,
                     "idle_channels": 1, "left_secs_ago": null, "local": false, "busy_total": 0, "at_limit": false}],
        "services": [{"name": "web", "protocol": "tcp", "bind": "0.0.0.0:8080", "target": "127.0.0.1:3000",
                      "clients": ["home"], "mode": null, "bandwidth": null, "visitors": [], "proxy_protocol": null,
                      "filtered": false, "open": 0, "total": 0, "refused": 0, "bytes_in": 0, "bytes_out": 0,
                      "health": [{"client": "home", "up": true}], "set_aside": []}],
        "visitors": [{"name": "ops", "open": 0, "total": 1, "last_seen_secs": 2}]
    }))
    .unwrap();
    assert!(st.web.is_none() && st.certificates.is_empty());
    assert_eq!(st.clients[0].allow, None);
    let p: Plan = serde_json::from_value(json!({
        "plan": "b2s:9", "base": "b2s:1", "result_rev": "b2s:2", "valid": true,
        "diags": [], "changes": [], "diff": {"unified": "", "hunks": []}, "impacts": [], "refusals": [], "applies": [],
        "secrets_in_diff": ""
    }))
    .unwrap();
    assert!(p.can_apply());
    let a: Applied = serde_json::from_value(
        json!({"rev": "b2s:2", "applied": [], "restart_needed": [], "secrets": []}),
    )
    .unwrap();
    assert!(a.not_listening.is_empty());
    let h: HistoryReply = serde_json::from_value(json!({"entries": [{
        "at": 1, "visitor": "ops", "from": "1.2.3.4", "base": "b2s:1", "rev": "b2s:2",
        "changes": [], "result": "applied", "revertible": true, "details_kept": true
    }]}))
    .unwrap();
    assert_eq!(h.entries[0].id, 0, "a v0.1.2 entry has no id");
}

#[test]
fn what_a_v0_1_2_ui_sends_still_decodes() {
    let r: HistoryRequest = serde_json::from_value(json!({"limit": 1, "diff": "b2s:2"})).unwrap();
    assert_eq!(r.diff.as_deref(), Some("b2s:2"));
    assert_eq!(r.entry, None);
    let r: HistoryRequest = serde_json::from_value(json!({"limit": 20})).unwrap();
    assert_eq!((r.diff, r.entry), (None, None));
    let e = AuditEntry::default();
    let sent = serde_json::to_value(&e).unwrap();
    assert!(sent.get("rev").is_some() && sent.get("id").is_some());
}
