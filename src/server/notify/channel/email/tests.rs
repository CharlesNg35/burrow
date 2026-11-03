use super::*;
use crate::config::settings::SecretText;
use crate::server::notify::channel::testing::fake_smtp;
use crate::server::notify::tests::letter;
use burrow_config::NotifyEvent;

fn settings(port: u16) -> EmailSettings {
    EmailSettings {
        to: "ops@example.com".into(),
        from: "alerts@example.com".into(),
        host: "127.0.0.1".into(),
        port,
        security: SmtpSecurity::StartTlsIfOffered,
        user: Some("alerts@example.com".into()),
        password: Some(SecretText::new("pw".into())),
    }
}

#[tokio::test]
async fn an_email_goes_through_the_mail_server_with_its_login() {
    let (port, seen) = fake_smtp(b"235 ok\r\n").await;
    let email = Email::new(&settings(port)).unwrap();
    email
        .send(&letter(NotifyEvent::ClientOffline, false))
        .await
        .unwrap();
    drop(email);
    let got = seen.await.unwrap();
    let has = |p: &str| got.iter().any(|l| l.starts_with(p));
    assert!(has("AUTH PLAIN"), "{got:?}");
    assert!(has("MAIL FROM:<alerts@example.com>"), "{got:?}");
    assert!(has("RCPT TO:<ops@example.com>"), "{got:?}");
    assert!(
        has("Subject: [tunnel.example.com] Client home is offline"),
        "{got:?}"
    );
}

#[tokio::test]
async fn a_wrong_password_is_not_tried_again_and_a_busy_server_is() {
    let (port, _seen) = fake_smtp(b"535 5.7.8 bad credentials\r\n").await;
    let err = Email::new(&settings(port))
        .unwrap()
        .send(&letter(NotifyEvent::ClientOffline, false))
        .await
        .unwrap_err();
    assert!(!err.retryable(), "{err}");
    let (port, _seen) = fake_smtp(b"454 4.7.0 try later\r\n").await;
    let err = Email::new(&settings(port))
        .unwrap()
        .send(&letter(NotifyEvent::ClientOffline, false))
        .await
        .unwrap_err();
    assert!(err.retryable(), "{err}");
}

#[tokio::test]
async fn a_mail_server_that_is_not_there_is_tried_again() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let err = Email::new(&settings(port))
        .unwrap()
        .send(&letter(NotifyEvent::ClientOffline, false))
        .await
        .unwrap_err();
    assert!(err.retryable(), "{err}");
}
