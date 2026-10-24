use std::time::Duration;

use burrow_admin_proto::msg::ErrorCode;
use serde_json::json;

use super::{connect, world, world_checked};

#[tokio::test]
async fn burrow_admin_reduces_admin_power_but_never_grants_it() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    let (once, refused) = (
        Arc::new(AtomicBool::new(true)),
        Arc::new(AtomicBool::new(false)),
    );
    let (once_, refused_) = (once.clone(), refused.clone());
    let w = world_checked(Box::new(
        move |next: burrow::config::settings::ServerSettings| {
            let granted = next
                .visitors
                .get("plain")
                .is_some_and(|v| v.admin.is_some());
            if granted && once_.swap(false, Ordering::SeqCst) {
                refused_.store(true, Ordering::SeqCst);
                return Err("the test refuses the first reload with the grant".into());
            }
            Ok(next)
        },
    ))
    .await;
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    let before = std::fs::read_to_string(&w.path).unwrap();
    let grant = json!([{"op": "set", "path": ["visitors", "plain", "admin"], "value": "write"}]);
    let plan = a.plan(grant.clone()).await.unwrap();
    assert_eq!(plan["refusals"][0]["code"], "admin-grant", "{plan}");
    assert_eq!(
        plan["refusals"][0]["message"],
        format!(
            "admin access is granted only in the config file: visitor \"plain\" would get admin: write; use burrow edit -c {} or edit the file",
            w.path.display()
        )
    );
    let err = a.apply(grant, &["plain"]).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::LockedOut, "{err:?}");
    assert_eq!(
        std::fs::read_to_string(&w.path).unwrap(),
        before,
        "nothing was written"
    );

    std::fs::write(
        &w.path,
        before.replace("  plain:\n", "  plain:\n    admin: write\n"),
    )
    .unwrap();
    let started = std::time::Instant::now();
    while !refused.load(Ordering::SeqCst) {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the hand edit was reloaded"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    a.apply(
        json!([{"op": "set", "path": ["services", "web", "target"], "value": 3002}]),
        &[],
    )
    .await
    .expect("an unapplied grant from the file does not block another change");

    let applied = a
        .apply(
            json!([{"op": "remove", "path": ["visitors", "plain", "admin"]}]),
            &[],
        )
        .await
        .expect("revoking another admin's write access applies");
    let revoked = std::fs::read_to_string(&w.path).unwrap();
    assert_eq!(revoked, before.replace("target: 3000", "target: 3002"));
    let history = a.call("history", json!({})).await.unwrap();
    let entry = history["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["rev"] == applied["rev"])
        .unwrap()
        .clone();
    let to = entry["base"].as_str().unwrap().to_string();
    let rev = a.state().await["rev"].clone();
    let plan = a
        .call("revert", json!({"to": to, "base": rev}))
        .await
        .unwrap();
    assert_eq!(plan["refusals"][0]["code"], "admin-grant", "{plan}");
    let err = a
        .call(
            "apply",
            json!({"base": plan["base"], "ops": [{"op": "replace_document", "from_history": to}], "plan": plan["plan"], "confirm": ["plain"]}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::LockedOut, "{err:?}");
    assert_eq!(
        std::fs::read_to_string(&w.path).unwrap(),
        revoked,
        "the revert wrote nothing"
    );
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn renaming_an_admin_visitor_is_refused_as_a_rename() {
    let w = world().await;
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    let before = std::fs::read_to_string(&w.path).unwrap();
    let rename =
        json!([{"op": "rename", "path": ["visitors", "viewer"], "to": "watcher", "refs": true}]);
    let plan = a.plan(rename.clone()).await.unwrap();
    assert_eq!(plan["refusals"][0]["code"], "admin-grant", "{plan}");
    assert_eq!(
        plan["refusals"][0]["message"],
        format!(
            "an admin visitor is renamed only in the config file: visitor \"viewer\" would become \"watcher\"; use burrow edit -c {} or edit the file",
            w.path.display()
        )
    );
    let err = a.apply(rename, &["viewer"]).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::LockedOut, "{err:?}");
    assert_eq!(std::fs::read_to_string(&w.path).unwrap(), before);
    let _ = w.stop.send(true);
}
