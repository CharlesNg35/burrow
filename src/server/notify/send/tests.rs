use super::*;
use crate::config::settings::SecretText;
use crate::server::notify::tests::letter;
use burrow_config::NotifyEvent;
use http_body_util::BodyExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

type Seen = (http::request::Parts, Vec<u8>);

async fn one_request(answer: u16) -> (String, tokio::task::JoinHandle<Seen>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", l.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (stream, _) = l.accept().await.unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let tx = std::sync::Mutex::new(Some(tx));
        let service =
            hyper::service::service_fn(move |req: http::Request<hyper::body::Incoming>| {
                let tx = tx.lock().unwrap().take();
                async move {
                    let (parts, body) = req.into_parts();
                    let body = body.collect().await.unwrap().to_bytes().to_vec();
                    if let Some(tx) = tx {
                        let _ = tx.send((parts, body));
                    }
                    let resp = http::Response::builder()
                        .status(answer)
                        .body(http_body_util::Full::new(bytes::Bytes::new()))
                        .unwrap();
                    Ok::<_, std::convert::Infallible>(resp)
                }
            });
        let _ = hyper::server::conn::http1::Builder::new()
            .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
            .await;
        rx.await.unwrap()
    });
    (url, task)
}

#[tokio::test]
async fn a_webhook_gets_the_message_as_json() {
    let (url, seen) = one_request(204).await;
    let sender = Sender::new(&NotifyVia::Webhook(url)).unwrap();
    sender
        .send(&letter(NotifyEvent::ClientOffline, false))
        .await
        .unwrap();
    let (parts, body) = seen.await.unwrap();
    assert_eq!(parts.method, Method::POST);
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
async fn ntfy_gets_the_text_with_a_title_priority_and_tag() {
    let (url, seen) = one_request(200).await;
    let sender = Sender::new(&NotifyVia::Ntfy(url)).unwrap();
    sender
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
    let problem = ntfy_request(
        "https://ntfy.sh/t",
        &letter(NotifyEvent::ClientOffline, false),
    );
    let problem = problem.unwrap();
    assert_eq!(problem.headers()["priority"], "high");
    assert_eq!(problem.headers()["tags"], "warning");
}

#[tokio::test]
async fn an_error_answer_is_a_failure_that_names_the_status() {
    let (url, _seen) = one_request(500).await;
    let sender = Sender::new(&NotifyVia::Webhook(url)).unwrap();
    let err = sender
        .send(&letter(NotifyEvent::ClientOffline, false))
        .await
        .unwrap_err();
    assert_eq!(err, "the server answered 500 Internal Server Error");
}

#[tokio::test]
async fn plain_http_to_a_public_address_is_refused_at_send() {
    let sender = Sender::new(&NotifyVia::Webhook("http://192.0.2.1/x".into())).unwrap();
    let err = sender
        .send(&letter(NotifyEvent::ClientOffline, false))
        .await
        .unwrap_err();
    assert!(err.contains("public address"), "{err}");
}

async fn fake_smtp() -> (u16, tokio::task::JoinHandle<Vec<String>>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        let (stream, _) = l.accept().await.unwrap();
        let (r, mut w) = stream.into_split();
        let mut lines = BufReader::new(r).lines();
        let mut got = Vec::new();
        w.write_all(b"220 fake ESMTP\r\n").await.unwrap();
        let mut data = false;
        while let Some(line) = lines.next_line().await.unwrap() {
            got.push(line.clone());
            let reply: &[u8] = match line.as_str() {
                "." if data => {
                    data = false;
                    b"250 queued\r\n"
                }
                _ if data => continue,
                l if l.starts_with("EHLO") => b"250-fake\r\n250 AUTH PLAIN LOGIN\r\n",
                l if l.starts_with("AUTH") => b"235 ok\r\n",
                "DATA" => {
                    data = true;
                    b"354 go\r\n"
                }
                "QUIT" => {
                    w.write_all(b"221 bye\r\n").await.unwrap();
                    break;
                }
                _ => b"250 ok\r\n",
            };
            w.write_all(reply).await.unwrap();
        }
        got
    });
    (port, task)
}

#[tokio::test]
async fn an_email_goes_through_the_mail_server_with_its_login() {
    let (port, seen) = fake_smtp().await;
    let mail = EmailSettings {
        to: "ops@example.com".into(),
        from: "alerts@example.com".into(),
        host: "127.0.0.1".into(),
        port,
        security: SmtpSecurity::StartTlsIfOffered,
        user: Some("alerts@example.com".into()),
        password: Some(SecretText::new("pw".into())),
    };
    let sender = Sender::new(&NotifyVia::Email(mail)).unwrap();
    sender
        .send(&letter(NotifyEvent::ClientOffline, false))
        .await
        .unwrap();
    drop(sender);
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
