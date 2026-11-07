use serde_json::{Value, json};

use super::{connect, world};

fn codes(plan: &Value, key: &str) -> Vec<String> {
    plan[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["code"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn tuning_and_trust_settings_are_set_from_the_admin() {
    let w = world().await;
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    for ops in [
        json!([{"op": "set", "path": ["heartbeat"], "value": {"interval_secs": 20}}]),
        json!([{"op": "set", "path": ["limits", "handshake_timeout_secs"], "value": 9}]),
        json!([{"op": "set", "path": ["limits", "max_handshakes"], "value": 4096}]),
    ] {
        let plan = a
            .plan(ops.clone())
            .await
            .unwrap_or_else(|e| panic!("{ops}: {e:?}"));
        assert_eq!(plan["refusals"], json!([]), "{ops}: {plan}");
        assert_eq!(plan["valid"], true, "{ops}: {plan}");
        a.apply(ops.clone(), &[])
            .await
            .unwrap_or_else(|e| panic!("{ops}: {e:?}"));
    }
    let text = std::fs::read_to_string(&w.path).unwrap();
    assert!(text.contains("interval_secs: 20"), "{text}");
    assert!(text.contains("max_handshakes: 4096"), "{text}");
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn turning_off_encryption_is_a_danger_to_confirm() {
    let w = world().await;
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    let ops = json!([{"op": "set", "path": ["insecure_plaintext"], "value": true}]);
    let plan = a.plan(ops.clone()).await.unwrap();
    assert_eq!(plan["refusals"], json!([]), "{plan}");
    let toggled = plan["impacts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["code"] == "plaintext-toggled")
        .unwrap_or_else(|| panic!("{plan}"))
        .clone();
    assert_eq!(toggled["level"], "danger");
    assert_eq!(toggled["confirm"], "insecure_plaintext");
    assert!(
        a.apply(ops.clone(), &[]).await.is_err(),
        "needs its confirm"
    );
    a.apply(ops, &["insecure_plaintext"]).await.unwrap();
    assert!(
        std::fs::read_to_string(&w.path)
            .unwrap()
            .contains("insecure_plaintext: true")
    );
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn the_tunnel_listener_moves_after_a_test_bind_and_a_confirm() {
    if super::common::ws_mode() {
        return;
    }
    let w = world().await;
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let busy = held.local_addr().unwrap().to_string();
    let plan = a
        .plan(json!([{"op": "set", "path": ["listen"], "value": busy}]))
        .await
        .unwrap();
    assert_eq!(plan["valid"], false, "{plan}");
    assert!(
        codes(&plan, "diags").contains(&"port-in-use".to_string()),
        "{plan}"
    );
    let free = format!("127.0.0.1:{}", super::common::free_port());
    let ops = json!([{"op": "set", "path": ["listen"], "value": free}]);
    let plan = a.plan(ops.clone()).await.unwrap();
    assert_eq!(plan["valid"], true, "{plan}");
    assert_eq!(plan["refusals"], json!([]), "{plan}");
    assert!(
        codes(&plan, "impacts").contains(&"changes-listen".to_string()),
        "{plan}"
    );
    a.apply(ops, &["listen"]).await.unwrap();
    assert!(
        std::fs::read_to_string(&w.path)
            .unwrap()
            .contains(&format!("listen: {free}"))
    );
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn the_access_log_goes_only_into_the_servers_own_folders_from_here() {
    let w = world().await;
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    let web = |log: &str| json!([{"op": "set", "path": ["web"], "value": {"http": format!("127.0.0.1:{}", super::common::free_port()), "access_log": log}}]);
    let outside = w.path.parent().unwrap().join(".bashrc");
    let plan = a.plan(web(&outside.display().to_string())).await.unwrap();
    assert_eq!(plan["valid"], false, "{plan}");
    let d = plan["diags"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["code"] == "access-log-outside")
        .unwrap_or_else(|| panic!("{plan}"))
        .clone();
    assert_eq!(d["path"], json!(["web", "access_log"]));
    assert!(
        d["message"].as_str().unwrap().contains("server.yaml"),
        "{d}"
    );
    let state = burrow::install::state_dir_for(&w.path);
    let inside = state.join("access.log").display().to_string();
    let plan = a.plan(web(&inside)).await.unwrap();
    assert!(
        !codes(&plan, "diags").contains(&"access-log-outside".to_string()),
        "{plan}"
    );
    let _ = w.stop.send(true);
}
