use std::net::SocketAddr;
use std::time::Duration;

use burrow::dialer::admin::{AdminOpenError, AdminRefusal};
use burrow::wire::secret::Secret;
use burrow_admin_proto::msg::ErrorCode;
use serde_json::{Value, json};

use super::common;
use super::common::free_port;
use super::{World, connect, world, world_checked};

#[tokio::test]
async fn an_admin_gets_welcome_schema_state_and_live_status() {
    let w = world().await;
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    let welcome = a.call("welcome", json!({})).await.unwrap();
    assert_eq!(welcome["you"]["visitor"], "ops");
    assert_eq!(welcome["you"]["level"], "write");
    assert_eq!(welcome["protocol"], 1);
    assert_eq!(
        welcome["server"]["name_hint"], "",
        "without public_address there is no name hint, never the host name"
    );
    assert!(
        welcome["features"]
            .as_array()
            .unwrap()
            .contains(&json!("history"))
    );
    let schema = a.call("schema", json!({"have": ""})).await.unwrap();
    assert_eq!(schema["hash"], welcome["schema_hash"]);
    assert_eq!(schema["schema"]["schema_version"], 1);
    let same = a
        .call("schema", json!({"have": welcome["schema_hash"]}))
        .await
        .unwrap();
    assert_eq!(same["unchanged"], true);
    let state = a.state().await;
    assert!(state["rev"].as_str().unwrap().starts_with("b2s:"));
    assert_eq!(
        state["values"]["visitors"]["ops"]["secret"]["$secret"]["fp"],
        w.ops.fingerprint()
    );
    assert!(!state.to_string().contains(&w.ops.encode()));
    assert!(state["file"]["writable"].as_bool().unwrap());
    a.call("subscribe", json!({"status_ms": 500}))
        .await
        .unwrap();
    let first = a.event("status").await;
    let second = a.event("status").await;
    assert_eq!(first["clients"][0]["name"], "home");
    assert!(second["uptime_secs"].is_u64());
    let unknown = a.call("teleport", json!({})).await.unwrap_err();
    assert_eq!(unknown.code, ErrorCode::UnknownRequest);
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn only_admin_visitors_get_in_and_sessions_are_capped() {
    let w = world().await;
    let refused = connect(&w, "plain", &w.plain, 1, 1).await.err().unwrap();
    assert!(
        matches!(refused, AdminOpenError::Refused(AdminRefusal::Forbidden)),
        "{refused:?}"
    );
    let wrong = connect(&w, "ops", &w.plain, 1, 1).await.err().unwrap();
    assert!(matches!(wrong, AdminOpenError::Dial(_)), "{wrong:?}");
    let old = connect(&w, "ops", &w.ops, 2, 3).await.err().unwrap();
    match old {
        AdminOpenError::Refused(AdminRefusal::Explained(t)) => {
            assert!(t.contains("versions 1 to 1"), "{t}")
        }
        other => panic!("{other:?}"),
    }
    let a = connect(&w, "viewer", &w.viewer, 1, 1).await.unwrap();
    let b = connect(&w, "viewer", &w.viewer, 1, 1).await.unwrap();
    let busy = connect(&w, "viewer", &w.viewer, 1, 1).await.err().unwrap();
    assert!(
        matches!(busy, AdminOpenError::Refused(AdminRefusal::Busy)),
        "{busy:?}"
    );
    drop((a, b));
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn read_access_cannot_change_anything() {
    let w = world().await;
    let mut v = connect(&w, "viewer", &w.viewer, 1, 1).await.unwrap();
    let state = v.state().await;
    assert_eq!(state["file"]["writable"], false);
    assert_eq!(
        state["file"]["why_not"]["code"],
        burrow_admin_proto::codes::READ_LEVEL
    );
    let e = v.plan(json!([])).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::Forbidden);
    let e = v.call("generate_secret", json!({})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::Forbidden);
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn validation_errors_name_their_fields() {
    let w = world().await;
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    let state = a.state().await;
    let bind = state["values"]["services"]["web"]["bind"].clone();
    let plan = a
        .plan(json!([{"op": "add", "path": ["services", "api"], "value": {"client": "home", "bind": bind, "target": 3001}}]))
        .await
        .unwrap();
    assert_eq!(plan["valid"], false);
    let d = plan["diags"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["code"] == "duplicate-bind")
        .expect("duplicate-bind")
        .clone();
    let mut paths = vec![d["path"].clone()];
    paths.extend(d["also"].as_array().unwrap().iter().cloned());
    assert!(paths.contains(&json!(["services", "api", "bind"])), "{d}");
    assert!(paths.contains(&json!(["services", "web", "bind"])), "{d}");
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn an_applied_change_keeps_comments_reloads_and_serves() {
    let w = world().await;
    let echo = common::echo_server().await;
    let mut client = common::client_settings(w.addr, w.home.clone());
    client.allow =
        burrow::policy::allowlist::AllowList::Rules(vec!["127.0.0.0/8".parse().unwrap()]);
    let mut running = common::start_client(client).await;
    common::wait_state(&mut running.state, burrow::client::State::Connected).await;
    let before = std::fs::read_to_string(&w.path).unwrap();
    let port = free_port();
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    let applied = a
        .apply(
            json!([{"op": "add", "path": ["services", "echo"], "value": {"client": "home", "bind": format!("127.0.0.1:{port}"), "target": echo.to_string()}}]),
            &[],
        )
        .await
        .unwrap();
    assert!(applied["rev"].as_str().unwrap().starts_with("b2s:"));
    let after = std::fs::read_to_string(&w.path).unwrap();
    assert!(
        after.starts_with(&before),
        "the old text stays byte for byte:\n{after}"
    );
    assert!(after.contains("# the home box") && after.contains("} # keep"));
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let mut ok = false;
    for _ in 0..100 {
        if common::try_roundtrip(addr, b"hello").await.as_deref() == Some(b"hello".as_slice()) {
            ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(ok, "the new service serves through the tunnel");
    let history = a.call("history", json!({})).await.unwrap();
    assert_eq!(history["entries"][0]["visitor"], "ops");
    assert!(
        history["entries"][0]["changes"][0]["summary"]
            .as_str()
            .unwrap()
            .contains("echo")
    );
    let _ = running.stop.send(true);
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn a_hand_edit_in_between_is_a_conflict() {
    let w = world().await;
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    let ops = json!([{"op": "set", "path": ["services", "web", "target"], "value": 3002}]);
    let plan = a.plan(ops.clone()).await.unwrap();
    let text = std::fs::read_to_string(&w.path).unwrap();
    let other = Secret::generate();
    let edited = text.replace(
        "\nvisitors:",
        &format!("  office:\n    secret: {}\n\nvisitors:", other.encode()),
    );
    std::fs::write(&w.path, &edited).unwrap();
    let e = a
        .call(
            "apply",
            json!({"base": plan["base"], "ops": ops, "plan": plan["plan"], "confirm": []}),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Conflict);
    assert_ne!(e.body["current"], plan["base"]);
    assert_eq!(
        std::fs::read_to_string(&w.path).unwrap(),
        edited,
        "nothing is written on a conflict"
    );
    let started = std::time::Instant::now();
    let retry = loop {
        let plan = a.plan(ops.clone()).await.unwrap();
        let live = !plan["impacts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["code"] == burrow_admin_proto::codes::ADDS_CLIENT);
        if live {
            break plan;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the hand-edited client never became live: {plan}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    a.call(
        "apply",
        json!({"base": retry["base"], "ops": ops, "plan": retry["plan"], "confirm": []}),
    )
    .await
    .unwrap();
    let last = std::fs::read_to_string(&w.path).unwrap();
    assert!(last.contains("office:") && last.contains("target: 3002"));
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn a_crafted_change_of_a_locked_field_is_refused_and_nothing_is_written() {
    let w = world().await;
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    let before = std::fs::read_to_string(&w.path).unwrap();
    for ops in [
        json!([{"op": "set", "path": ["listen"], "value": 7999}]),
        json!([{"op": "set", "path": ["insecure_plaintext"], "value": true}]),
        json!([{"op": "set", "path": ["websocket", "tls"], "value": {"cert": "a.pem", "key": "a.key"}}]),
    ] {
        let e = a.plan(ops.clone()).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::LockedField, "{ops}");
        let rev = a.state().await["rev"].clone();
        let e = a
            .call(
                "apply",
                json!({"base": rev, "ops": ops, "plan": "b2s:0", "confirm": []}),
            )
            .await
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::LockedField);
    }
    assert_eq!(std::fs::read_to_string(&w.path).unwrap(), before);
    let schema = a.call("schema", json!({})).await.unwrap();
    assert_eq!(
        schema["schema"]["root"]["sem"]["fields"][0]["node"]["locked"],
        true
    );
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn the_guard_refuses_every_lockout() {
    let w = world().await;
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    let before = std::fs::read_to_string(&w.path).unwrap();
    let cases = [
        (
            json!([{"op": "remove", "path": ["visitors", "ops", "admin"]}]),
            "self-demoted",
        ),
        (
            json!([{"op": "remove", "path": ["visitors", "ops"]}]),
            "self-removed",
        ),
        (
            json!([{"op": "set", "path": ["visitors", "ops", "admin_from"], "value": ["10.0.0.0/8"]}]),
            "self-source",
        ),
        (
            json!([{"op": "set", "path": ["visitors", "ops", "admin"], "value": "read"}]),
            "self-demoted",
        ),
    ];
    for (ops, code) in cases {
        let plan = a.plan(ops.clone()).await.unwrap();
        assert!(
            plan["refusals"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["code"] == code),
            "{code}: {plan}"
        );
        let e = a
            .call(
                "apply",
                json!({"base": plan["base"], "ops": ops, "plan": plan["plan"], "confirm": ["ops"]}),
            )
            .await
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::LockedOut, "{code}");
    }
    assert_eq!(std::fs::read_to_string(&w.path).unwrap(), before);
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn losing_admin_rights_ends_the_session() {
    let w = world().await;
    let mut v = connect(&w, "viewer", &w.viewer, 1, 1).await.unwrap();
    v.call("subscribe", json!({"config": true})).await.unwrap();
    let text = std::fs::read_to_string(&w.path).unwrap();
    std::fs::write(&w.path, text.replace("    admin: read\n", "")).unwrap();
    let body = v.event("revoked").await;
    assert_eq!(body["reason"], "admin removed");
    let closed = tokio::time::timeout(Duration::from_secs(5), v.rx.recv())
        .await
        .unwrap();
    assert!(matches!(closed, Ok(None) | Err(_)));
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn a_changed_admin_level_ends_the_session() {
    let w = world().await;
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    a.call("subscribe", json!({"config": true})).await.unwrap();
    let mut v = connect(&w, "viewer", &w.viewer, 1, 1).await.unwrap();
    v.call("subscribe", json!({"config": true})).await.unwrap();
    let text = std::fs::read_to_string(&w.path).unwrap();
    let swapped = text
        .replace("    admin: write\n", "    admin: WAS_WRITE\n")
        .replace("    admin: read\n", "    admin: write\n")
        .replace("    admin: WAS_WRITE\n", "    admin: read\n");
    std::fs::write(&w.path, swapped).unwrap();
    for s in [&mut a, &mut v] {
        let body = s.event("revoked").await;
        assert_eq!(body["reason"], "admin level changed");
        let closed = tokio::time::timeout(Duration::from_secs(5), s.rx.recv())
            .await
            .unwrap();
        assert!(matches!(closed, Ok(None) | Err(_)), "{closed:?}");
    }
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    let err = a
        .plan(json!([{"op": "set", "path": ["visitors", "ops", "admin"], "value": "write"}]))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden, "{err:?}");
    let _ = w.stop.send(true);
}

async fn apply_and_close(w: &World, ops: Value) {
    let mut a = connect(w, "ops", &w.ops, 1, 1).await.unwrap();
    let plan = a.plan(ops.clone()).await.unwrap();
    assert_eq!(plan["valid"], true, "{plan}");
    a.send_raw(
        "apply",
        json!({"base": plan["base"], "ops": ops, "plan": plan["plan"], "confirm": []}),
    )
    .await;
    drop(a);
}

async fn wait_for_entry(w: &World, result: &str) -> Value {
    let started = std::time::Instant::now();
    loop {
        let mut v = connect(w, "viewer", &w.viewer, 1, 1).await.unwrap();
        let history = v.call("history", json!({})).await.unwrap();
        if let Some(e) = history["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["visitor"] == "ops" && e["result"] == result)
        {
            return e.clone();
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "no {result:?} entry: {history}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test]
async fn an_apply_runs_to_its_end_when_its_session_closes_at_once() {
    let w = world_checked(Box::new(
        |next: burrow::config::settings::ServerSettings| {
            if next.services.contains_key("refused") {
                Err("the test refuses this service".into())
            } else {
                Ok(next)
            }
        },
    ))
    .await;
    apply_and_close(
        &w,
        json!([{"op": "set", "path": ["services", "web", "target"], "value": 3002}]),
    )
    .await;
    wait_for_entry(&w, "applied").await;
    assert!(
        std::fs::read_to_string(&w.path)
            .unwrap()
            .contains("target: 3002")
    );
    let before = std::fs::read_to_string(&w.path).unwrap();
    apply_and_close(
        &w,
        json!([{"op": "add", "path": ["services", "refused"], "value": {"client": "home", "bind": format!("127.0.0.1:{}", free_port()), "target": 3005}}]),
    )
    .await;
    wait_for_entry(&w, "rejected, restored").await;
    assert_eq!(
        std::fs::read_to_string(&w.path).unwrap(),
        before,
        "the rejected change is written back"
    );
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn a_queued_apply_of_a_revoked_session_writes_nothing() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    let release = Arc::new(AtomicBool::new(false));
    let gate = release.clone();
    let w = world_checked(Box::new(
        move |mut next: burrow::config::settings::ServerSettings| {
            if next.services.contains_key("slow") {
                let started = std::time::Instant::now();
                while !gate.load(Ordering::SeqCst) && started.elapsed() < Duration::from_secs(20) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                return Err("the test refuses the slow service".into());
            }
            if gate.load(Ordering::SeqCst)
                && let Some(ops) = next.visitors.get_mut("ops")
            {
                ops.admin = Some(burrow_admin_proto::msg::AdminLevel::Read);
            }
            Ok(next)
        },
    ))
    .await;
    let mut b = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    let late = json!([{"op": "set", "path": ["services", "web", "target"], "value": 3009}]);
    let late_plan = b.plan(late.clone()).await.unwrap();
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    let first = json!([{"op": "add", "path": ["services", "slow"], "value": {"client": "home", "bind": format!("127.0.0.1:{}", free_port()), "target": 3005}}]);
    let first_plan = a.plan(first.clone()).await.unwrap();
    a.send_raw(
        "apply",
        json!({"base": first_plan["base"], "ops": first, "plan": first_plan["plan"], "confirm": []}),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    b.send_raw(
        "apply",
        json!({"base": late_plan["base"], "ops": late, "plan": late_plan["plan"], "confirm": []}),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    release.store(true, Ordering::SeqCst);
    wait_for_entry(&w, "rejected, restored").await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let text = std::fs::read_to_string(&w.path).unwrap();
    assert!(!text.contains("slow:"), "{text}");
    assert!(
        !text.contains("target: 3009"),
        "an apply of a session that lost write access was written:\n{text}"
    );
    drop((a, b));
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn an_oversized_message_closes_the_session() {
    let w = world().await;
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    a.call("ping", json!({})).await.unwrap();
    let big = vec![b'x'; 4 * 1024 * 1024 + 1];
    assert!(
        a.tx.send(&big).await.is_err(),
        "the client refuses to send it"
    );
    let chunk = vec![b'x'; burrow_admin_proto::frame::MAX_CHUNK];
    let mut sent = 0;
    while sent <= 4 * 1024 * 1024 {
        let mut rec = vec![burrow_admin_proto::frame::FLAG_MORE];
        rec.extend_from_slice(&chunk);
        if a.tx.send_record(&rec).await.is_err() {
            break;
        }
        sent += chunk.len();
    }
    let end = tokio::time::timeout(Duration::from_secs(5), a.rx.recv())
        .await
        .unwrap();
    assert!(matches!(end, Ok(None) | Err(_)), "{end:?}");
    let _ = w.stop.send(true);
}
