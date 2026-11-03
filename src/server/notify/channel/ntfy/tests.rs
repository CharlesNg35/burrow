use super::*;
use crate::server::notify::channel::testing::one_request;
use crate::server::notify::tests::letter;
use burrow_config::NotifyEvent;

#[tokio::test]
async fn ntfy_gets_the_text_with_a_title_priority_and_tag() {
    let (url, seen) = one_request(200).await;
    Ntfy::new(&url)
        .unwrap()
        .send(&letter(NotifyEvent::ClientBack, true))
        .await
        .unwrap();
    let (parts, body) = seen.await.unwrap();
    assert_eq!(
        parts.headers["title"],
        "[tunnel.example.com] Client home is offline"
    );
    assert_eq!(parts.headers["priority"], "default");
    assert_eq!(parts.headers["tags"], "white_check_mark");
    assert_eq!(body, b"Client \"home\" lost its connection.");
    let problem = request(
        "https://ntfy.sh/t",
        &letter(NotifyEvent::ClientOffline, false),
    );
    let problem = problem.unwrap();
    assert_eq!(problem.headers()["priority"], "high");
    assert_eq!(problem.headers()["tags"], "warning");
}

#[tokio::test]
async fn a_busy_ntfy_server_is_tried_again() {
    let (url, _seen) = one_request(429).await;
    let err = Ntfy::new(&url)
        .unwrap()
        .send(&letter(NotifyEvent::ClientOffline, false))
        .await
        .unwrap_err();
    assert!(err.retryable(), "{err}");
}

#[test]
fn a_title_outside_ascii_is_kept_readable_in_its_header() {
    assert_eq!(ascii("café ✓"), "caf? ?");
}
