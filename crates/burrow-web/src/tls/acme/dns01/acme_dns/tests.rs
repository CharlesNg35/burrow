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

#[tokio::test]
async fn plain_http_reaches_only_a_loopback_or_private_api() {
    let a = Api::start_plain(Arc::new(|_: &Seen| (StatusCode::OK, json!({})))).await;
    assert!(a.url.starts_with("http://127.0.0.1:"), "{}", a.url);
    let plain = || {
        crate::tls::acme::http::Client::new(None, crate::tls::acme::http::Limits::default())
            .unwrap()
    };
    let target = name(&format!("{SUB}.auth.example.org")).unwrap();
    let local = AcmeDns::new(
        &a.url,
        "user",
        Secret::new("pw".into()),
        SUB,
        plain().with_plain_private(),
    );
    local.present(&target, "v").await.unwrap();
    assert_eq!(a.seen().len(), 1);
    let public = AcmeDns::new(
        "http://192.0.2.10:8053",
        "user",
        Secret::new("pw".into()),
        SUB,
        plain().with_plain_private(),
    );
    let err = public.present(&target, "v").await.unwrap_err();
    assert_eq!(
        err.text(),
        "acme-dns update: 192.0.2.10 is a public address; plain http:// is allowed only to loopback or private addresses (10/8, 172.16/12, 192.168/16, fc00::/7), so use https://"
    );
    let strict = AcmeDns::new(&a.url, "user", Secret::new("pw".into()), SUB, plain());
    let err = strict.present(&target, "v").await.unwrap_err();
    assert!(
        err.text().ends_with("only https is allowed"),
        "{}",
        err.text()
    );
    assert_eq!(a.seen().len(), 1);
}

#[test]
fn private_means_loopback_rfc_1918_or_unique_local() {
    use crate::tls::acme::http::is_private;
    for ip in [
        "127.0.0.1",
        "10.1.2.3",
        "172.16.0.1",
        "172.31.255.255",
        "192.168.1.2",
        "::1",
        "fd12:3456::1",
        "::ffff:192.168.1.2",
    ] {
        assert!(is_private(ip.parse().unwrap()), "{ip}");
    }
    for ip in [
        "8.8.8.8",
        "172.32.0.1",
        "100.64.0.1",
        "169.254.1.1",
        "192.0.2.10",
        "fe80::1",
        "2001:db8::1",
        "::ffff:8.8.8.8",
    ] {
        assert!(!is_private(ip.parse().unwrap()), "{ip}");
    }
}
