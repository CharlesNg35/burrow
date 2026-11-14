use serde_json::json;

use super::common::admin::world;
use super::{ai, visitor};

#[tokio::test]
async fn check_change_offers_no_replace_text() {
    let w = world().await;
    let v = visitor(&w, "ops", &w.ops);
    let a = ai(&v, &[]).await;
    let tools = a.client.list_all_tools().await.unwrap();
    let check = tools.iter().find(|t| t.name == "check_change").unwrap();
    let schema = serde_json::to_string(&check.input_schema).unwrap();
    assert!(schema.contains("secret_add"), "{schema}");
    assert!(!schema.contains("replace_text"), "{schema}");
    a.stop().await;
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn check_change_refuses_replace_text() {
    let w = world().await;
    let v = visitor(&w, "ops", &w.ops);
    let a = ai(&v, &[]).await;
    let before = std::fs::read_to_string(&w.path).unwrap();
    let ops = json!({ "ops": [{
        "op": "replace_text",
        "files": [{ "name": "server.yaml", "text": "listen: 7835\n" }]
    }] });
    let (failed, refused) = a.call("check_change", ops).await;
    assert!(failed, "{refused}");
    assert_eq!(refused["error"], "text-op", "{refused}");
    let said = refused["message"].as_str().unwrap_or_default();
    assert!(
        said.contains("replace_text is for burrow admin's text editor; use the other ops"),
        "{refused}"
    );
    assert_eq!(std::fs::read_to_string(&w.path).unwrap(), before);
    a.stop().await;
    let _ = w.stop.send(true);
}
