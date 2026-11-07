use serde_json::json;

use super::common::admin::world;
use super::{ai, visitor};

#[tokio::test]
async fn a_new_client_gets_a_secret_the_ai_never_sees_and_its_config_lands_in_a_private_file() {
    let w = world().await;
    let v = visitor(&w, "ops", &w.ops);
    let a = ai(&v, &[]).await;
    let made = a.ok("new_secret", json!({})).await;
    let (handle, fp) = (made["handle"].clone(), made["fp"].clone());
    assert!(handle.is_string() && fp.is_string(), "{made}");
    let ops = json!({ "ops": [
        { "op": "add", "path": ["clients", "nas"], "value": {} },
        { "op": "secret_add", "path": ["clients", "nas", "secret"], "handle": handle }
    ] });
    let plan = a.ok("check_change", ops).await;
    assert_eq!(plan["can_apply"], true, "{plan}");
    let applied = a
        .ok(
            "apply_change",
            json!({ "plan": plan["plan"], "confirm": plan["confirm"] }),
        )
        .await;
    let file = applied["files"][0]["file"].as_str().unwrap_or_default();
    assert_eq!(applied["files"][0]["fp"], fp, "{applied}");
    let config = std::fs::read_to_string(file).unwrap();
    assert!(config.contains("name: nas"), "{config}");
    let secret = config
        .lines()
        .find_map(|l| l.trim().strip_prefix("secret: "))
        .unwrap();
    let parsed = burrow::wire::secret::Secret::parse(secret).unwrap();
    assert_eq!(fp.as_str(), Some(parsed.fingerprint().as_str()));
    assert!(!applied.to_string().contains(secret), "{applied}");
    assert!(
        std::path::Path::new(file).starts_with(dunce::canonicalize(v.sessions()).unwrap()),
        "{file}"
    );
    let server = std::fs::read_to_string(&w.path).unwrap();
    assert!(server.contains("nas:"), "{server}");
    a.stop().await;
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn a_new_password_is_written_for_the_user_and_only_its_handle_reaches_the_ai() {
    let w = world().await;
    let v = visitor(&w, "ops", &w.ops);
    let a = ai(&v, &[]).await;
    let made = a.ok("new_password", json!({ "user": "alice" })).await;
    let file = made["file"].as_str().unwrap_or_default();
    let password = std::fs::read_to_string(file).unwrap();
    let password = password.trim();
    assert!(password.len() >= 20, "a long password");
    assert!(!made.to_string().contains(password), "{made}");
    let site = json!({
        "client": "home",
        "target": 3000,
        "web": { "hosts": ["site.example.com"], "tls": "off" }
    });
    let ops = json!({ "ops": [
        { "op": "set", "path": ["web"], "value": { "http": 0 } },
        { "op": "add", "path": ["services", "site"], "value": site },
        { "op": "password_set", "path": ["services", "site", "web", "basic_auth", "alice"], "handle": made["handle"] }
    ] });
    let plan = a.ok("check_change", ops).await;
    let diff = plan["diff"]["unified"].as_str().unwrap_or_default();
    assert!(diff.contains("alice"), "{plan}");
    assert!(!plan.to_string().contains(password), "{plan}");
    a.stop().await;
    let (failed, _) = ai(&v, &["--read-only"])
        .await
        .call("new_password", json!({ "user": "bob" }))
        .await;
    assert!(failed, "--read-only leaves out the tools that make secrets");
    let _ = w.stop.send(true);
}
