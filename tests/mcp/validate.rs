use std::process::Stdio;

use serde_json::{Value, json};

use super::app::{self, STOP};
use super::common::admin::world;
use super::visitor;

const OWNER_EXAMPLE: &str = "web:
  letsencrypt:
    email: ops@example.com
    dns:
      - domains:
          - example.com
        cloudflare:
          token: ${CF_TOKEN}
        wildcards:
          - example.com
";

const FLOW_UNQUOTED: &str = "web:
  letsencrypt:
    email: ops@example.com
    dns:
      - domains: [example.com]
        cloudflare: { token: ${CF_TOKEN} }
        wildcards: [\"*.example.com\"]
";

const REFUSAL: &str = "web.letsencrypt.dns[0].wildcards: \"example.com\" is not a wildcard; write it as \"*.example.com\"";

fn messages(v: &Value) -> Vec<String> {
    v["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("{v}"))
        .iter()
        .map(|d| d["message"].as_str().unwrap_or_default().to_string())
        .collect()
}

async fn cli_messages(text: &str) -> Vec<String> {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("server.yaml");
    burrow::base::fs::write_private(&file, text).unwrap();
    let out = tokio::process::Command::new(env!("CARGO_BIN_EXE_burrow"))
        .args(["validate", "--role", "server", "--format", "json", "-c"])
        .arg(&file)
        .stdin(Stdio::null())
        .output()
        .await
        .unwrap();
    messages(&serde_json::from_slice(&out.stdout).unwrap())
}

#[tokio::test]
async fn a_read_only_admin_checks_the_owner_s_example_and_gets_the_validate_refusal() {
    let w = world().await;
    let before = std::fs::read_to_string(&w.path).unwrap();
    let v = visitor(&w, "viewer", &w.viewer);
    let mut a = app::mcp(&v.config, &[], &v.sessions());
    let info = a.ready().await;
    let text = info["instructions"].as_str().unwrap_or_default();
    assert!(text.contains("check it with validate_config"), "{text}");
    assert!(a.tools().await.iter().any(|t| t == "validate_config"));
    let (failed, got) = a
        .call("validate_config", json!({ "config": OWNER_EXAMPLE }))
        .await;
    assert!(!failed, "{got}");
    assert_eq!(got["valid"], false, "{got}");
    let said = messages(&got);
    assert!(said.iter().any(|m| m == REFUSAL), "{got}");
    assert!(
        cli_messages(OWNER_EXAMPLE)
            .await
            .iter()
            .any(|m| m == REFUSAL),
        "burrow validate says the same"
    );
    let fixed = OWNER_EXAMPLE.replace(
        "        wildcards:\n          - example.com\n",
        "        wildcards:\n          - \"*.example.com\"\n  https: 443\n",
    );
    let (failed, good) = a.call("validate_config", json!({ "config": fixed })).await;
    assert!(!failed && good["valid"] == true, "{good}");
    let (failed, flow) = a
        .call("validate_config", json!({ "config": FLOW_UNQUOTED }))
        .await;
    assert!(!failed && flow["valid"] == false, "{flow}");
    assert_eq!(flow["diagnostics"][0]["code"], "syntax", "{flow}");
    let help = flow["diagnostics"][0]["help"].as_str().unwrap_or_default();
    assert!(help.contains("must be quoted"), "{flow}");
    let secret = w.home.encode();
    let leaky = format!("clients:\n  home: {{ secret: {secret} }}\nlisten: {secret}\n");
    let (_, echoed) = a
        .call(
            "validate_config",
            json!({ "config": leaky, "scope": "whole" }),
        )
        .await;
    assert!(!echoed.to_string().contains(&secret), "{echoed}");
    let (failed, big) = a
        .call("validate_config", json!({ "config": "#".repeat(70_000) }))
        .await;
    assert!(failed && big["error"] == "too-large", "{big}");
    a.close_stdin();
    a.exits_within(STOP).await;
    assert_eq!(
        std::fs::read_to_string(&w.path).unwrap(),
        before,
        "nothing is written"
    );
    let _ = w.stop.send(true);
}
