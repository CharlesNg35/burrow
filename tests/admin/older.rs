use serde_json::json;

use super::{connect, world};

#[tokio::test]
async fn a_v0_1_2_ui_still_reads_and_diffs_the_history_by_rev() {
    let w = world().await;
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    let welcome = a.call("welcome", json!({})).await.unwrap();
    let features = welcome["features"].as_array().unwrap();
    for f in ["history", "history.ids", "local-ca", "upload", "passwords"] {
        assert!(features.iter().any(|x| x == f), "{f}: {welcome}");
    }
    for target in [3001, 3002] {
        a.apply(
            json!([{"op": "set", "path": ["services", "web", "target"], "value": target}]),
            &[],
        )
        .await
        .unwrap();
    }
    let h = a.call("history", json!({"limit": 20})).await.unwrap();
    let newest = &h["entries"][0];
    let by_rev = a
        .call("history", json!({"limit": 1, "diff": newest["rev"]}))
        .await
        .unwrap();
    let by_id = a
        .call("history", json!({"limit": 1, "entry": newest["id"]}))
        .await
        .unwrap();
    assert_eq!(by_rev["diff"], by_id["diff"], "{by_rev}");
    assert!(by_rev["diff"]["unified"].as_str().unwrap().contains("3002"));
    let e = a
        .call("history", json!({"diff": "b2s:0000000000000000"}))
        .await
        .unwrap_err();
    assert_eq!(e.code, burrow_admin_proto::msg::ErrorCode::BadRequest);
    let _ = w.stop.send(true);
}
