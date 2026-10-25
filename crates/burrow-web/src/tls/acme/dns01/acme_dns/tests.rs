use std::sync::Arc;

use http::StatusCode;
use serde_json::json;

use super::super::mock_api::{Api, Seen};
use super::super::wire::name;
use super::*;

const SUB: &str = "d420c923-bbd7-4056-ab64-c3ca54c9b3cf";

#[tokio::test]
async fn one_update_call_sets_the_value_and_cleanup_does_nothing() {
    let a = Api::start(Arc::new(|s: &Seen| {
        let ok = s.headers.get("x-api-key").and_then(|v| v.to_str().ok()) == Some("pw");
        if ok {
            (StatusCode::OK, json!({"txt": s.body["txt"]}))
        } else {
            (StatusCode::UNAUTHORIZED, json!({"error": "forbidden"}))
        }
    }))
    .await;
    let p = AcmeDns::new(
        &format!("{}/", a.url),
        "user",
        Secret::new("pw".into()),
        SUB,
        a.client(),
    );
    let target = name(&format!("{SUB}.auth.example.org")).unwrap();
    let value = "x".repeat(43);
    p.present(&target, &value).await.unwrap();
    p.cleanup(&target, &value).await.unwrap();
    let seen = a.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].path, "/update");
    assert_eq!(seen[0].body, json!({"subdomain": SUB, "txt": value}));
    assert_eq!(seen[0].headers["x-api-user"], "user");
    let bad = AcmeDns::new(&a.url, "user", Secret::new("wrong".into()), SUB, a.client());
    let err = bad.present(&target, &value).await.unwrap_err();
    assert_eq!(
        err.text(),
        "acme-dns update: HTTP 401 {\"error\":\"forbidden\"}"
    );
}

#[tokio::test]
async fn a_name_not_delegated_to_acme_dns_gets_the_cname_hint() {
    let a = Api::start(Arc::new(|_: &Seen| (StatusCode::OK, json!({})))).await;
    let p = AcmeDns::new(&a.url, "user", Secret::new("pw".into()), SUB, a.client());
    let err = p
        .present(&name("_acme-challenge.wiki.example.io").unwrap(), "v")
        .await
        .unwrap_err();
    assert!(err.text().starts_with("the record would go to _acme-challenge.wiki.example.io, but acme-dns answers for d420c923"), "{}", err.text());
    assert!(a.seen().is_empty());
}
