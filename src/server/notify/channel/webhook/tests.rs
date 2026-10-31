use super::*;
use crate::server::notify::channel::testing::one_request;
use crate::server::notify::tests::letter;
use burrow_config::NotifyEvent;

#[tokio::test]
async fn a_webhook_gets_the_message_as_json() {
    let (url, seen) = one_request(204).await;
    let hook = Webhook::new(&url).unwrap();
    hook.send(&letter(NotifyEvent::ClientOffline, false))
        .await
        .unwrap();
    let (parts, body) = seen.await.unwrap();
    assert_eq!(parts.method, ::http::Method::POST);
    assert_eq!(parts.uri.path(), "/hook");
    assert_eq!(parts.headers[CONTENT_TYPE], "application/json");
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        v,
        serde_json::json!({
            "event": "client_offline",
            "resolved": false,
            "kind": "client",
            "name": "home",
            "title": "[tunnel.example.com] Client home is offline",
            "text": "Client \"home\" lost its connection.",
            "server": "tunnel.example.com",
            "at": 1_793_448_000u64,
        })
    );
}

#[tokio::test]
async fn an_error_answer_is_a_failure_that_names_the_status() {
    let (url, _seen) = one_request(500).await;
    let err = Webhook::new(&url)
        .unwrap()
        .send(&letter(NotifyEvent::ClientOffline, false))
        .await
        .unwrap_err();
    assert_eq!(
        err,
        SendError::Retry("the server answered 500 Internal Server Error".into())
    );
}

#[tokio::test]
async fn a_refused_login_is_not_tried_again() {
    let (url, _seen) = one_request(403).await;
    let err = Webhook::new(&url)
        .unwrap()
        .send(&letter(NotifyEvent::ClientOffline, false))
        .await
        .unwrap_err();
    assert_eq!(
        err,
        SendError::Permanent("the server answered 403 Forbidden".into())
    );
}

#[tokio::test]
async fn plain_http_to_a_public_address_is_refused_at_send() {
    let err = Webhook::new("http://192.0.2.1/x")
        .unwrap()
        .send(&letter(NotifyEvent::ClientOffline, false))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("public address"), "{err}");
}
