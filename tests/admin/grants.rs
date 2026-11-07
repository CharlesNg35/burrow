use burrow_admin_proto::msg::ErrorCode;
use serde_json::json;

use super::{connect, world};

#[tokio::test]
async fn burrow_admin_grants_and_widens_admin_power() {
    let w = world().await;
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    for ops in [
        json!([{"op": "set", "path": ["visitors", "plain", "admin"], "value": "write"}]),
        json!([{"op": "set", "path": ["visitors", "viewer", "admin"], "value": "write"}]),
        json!([{"op": "set", "path": ["visitors", "viewer", "admin_from"], "value": ["127.0.0.0/8", "10.0.0.0/8"]}]),
        json!([{"op": "remove", "path": ["visitors", "viewer", "admin_from"]}]),
    ] {
        let plan = a.plan(ops.clone()).await.unwrap();
        assert_eq!(plan["refusals"], json!([]), "{ops}: {plan}");
        a.apply(ops.clone(), &[])
            .await
            .unwrap_or_else(|e| panic!("{ops}: {e:?}"));
    }
    let text = std::fs::read_to_string(&w.path).unwrap();
    assert!(
        text.contains("  plain:\n    secret: ") && text.contains("    admin: write\n"),
        "{text}"
    );
    assert!(!text.contains("admin: read"), "{text}");
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn an_admin_visitor_is_renamed_like_any_other() {
    let w = world().await;
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    let rename =
        json!([{"op": "rename", "path": ["visitors", "viewer"], "to": "watcher", "refs": true}]);
    let plan = a.plan(rename.clone()).await.unwrap();
    assert_eq!(plan["refusals"], json!([]), "{plan}");
    a.apply(rename, &[]).await.unwrap();
    assert!(
        std::fs::read_to_string(&w.path)
            .unwrap()
            .contains("  watcher:\n")
    );
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn your_own_secret_is_rotated_from_the_admin() {
    let w = world().await;
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    let fresh = a.call("generate_secret", json!({})).await.unwrap();
    let handle = fresh["handle"].clone();
    let add =
        json!([{"op": "secret_add", "path": ["visitors", "ops", "secret"], "handle": handle}]);
    let plan = a.plan(add.clone()).await.unwrap();
    assert_eq!(plan["refusals"], json!([]), "{plan}");
    assert_eq!(plan["impacts"], json!([]), "{plan}");
    a.apply(add, &[]).await.unwrap();
    let text = std::fs::read_to_string(&w.path).unwrap();
    assert!(text.contains(&w.ops.encode()), "{text}");
    let drop_own = json!([{"op": "secret_remove", "path": ["visitors", "ops", "secret"], "fp": w.ops.fingerprint()}]);
    let plan = a.plan(drop_own.clone()).await.unwrap();
    assert_eq!(plan["refusals"], json!([]), "{plan}");
    assert!(
        plan["impacts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["code"] == "self-secret" && i["confirm"] == "ops"),
        "{plan}"
    );
    let unconfirmed = a
        .call(
            "apply",
            json!({"base": plan["base"], "ops": drop_own, "plan": plan["plan"], "confirm": []}),
        )
        .await
        .unwrap_err();
    assert_ne!(unconfirmed.code, ErrorCode::LockedOut, "{unconfirmed:?}");
    a.call(
        "apply",
        json!({"base": plan["base"], "ops": drop_own, "plan": plan["plan"], "confirm": ["ops"]}),
    )
    .await
    .expect("the apply is answered before the session ends");
    assert!(
        !std::fs::read_to_string(&w.path)
            .unwrap()
            .contains(&w.ops.encode())
    );
    assert_eq!(a.event("revoked").await["reason"], "secret removed");
    let _ = w.stop.send(true);
}
