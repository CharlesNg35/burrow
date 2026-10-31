use std::path::PathBuf;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::pki_types::PrivateKeyDer;

use super::*;

struct Server {
    url: String,
    ca: PathBuf,
    head: oneshot::Receiver<String>,
    _dir: tempfile::TempDir,
}

async fn serve(answer: Vec<u8>, stall: bool) -> Server {
    let dir = tempfile::tempdir().unwrap();
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["localhost".to_string()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let ca = dir.path().join("ca.pem");
    std::fs::write(&ca, cert.pem()).unwrap();
    let provider = crate::tls::provider();
    let config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.der().clone()],
            PrivateKeyDer::try_from(key.serialize_der()).unwrap(),
        )
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, head) = oneshot::channel();
    tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut tls = TlsAcceptor::from(Arc::new(config))
            .accept(tcp)
            .await
            .unwrap();
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        while !got.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = tls.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            got.extend_from_slice(&buf[..n]);
        }
        let _ = tx.send(String::from_utf8_lossy(&got).into_owned());
        if stall {
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
        let _ = tls.write_all(&answer).await;
        let _ = tls.flush().await;
        let _ = tls.shutdown().await;
    });
    Server {
        url: format!("https://localhost:{port}/dir?x=1"),
        ca,
        head,
        _dir: dir,
    }
}

fn get(url: &str) -> Request<BodyWrapper<Bytes>> {
    Request::builder()
        .uri(url)
        .header(http::header::USER_AGENT, "instant-acme/0.8.5")
        .body(BodyWrapper::default())
        .unwrap()
}

fn answer(status: &str, extra: &str, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status}\r\ncontent-length: {}\r\n{extra}\r\n{body}",
        body.len()
    )
    .into_bytes()
}

#[tokio::test]
async fn the_request_has_a_host_and_an_origin_form_target_and_keeps_the_user_agent() {
    let s = serve(answer("200 OK", "", "{}"), false).await;
    let client = Client::new(Some(&s.ca), Limits::default()).unwrap();
    let resp = client.request(get(&s.url)).await.unwrap();
    assert_eq!(resp.parts.status, StatusCode::OK);
    let head = s.head.await.unwrap().to_ascii_lowercase();
    assert!(head.starts_with("get /dir?x=1 http/1.1\r\n"), "{head}");
    let port = s.url.split(':').nth(2).unwrap().split('/').next().unwrap();
    assert!(
        head.contains(&format!("host: localhost:{port}\r\n")),
        "{head}"
    );
    assert!(
        head.contains("user-agent: instant-acme/0.8.5\r\n"),
        "{head}"
    );
    assert_eq!(client.take_wait(), None);
}

#[tokio::test]
async fn an_absurd_retry_after_is_clamped_before_instant_acme_reads_it_and_recorded() {
    let s = serve(
        answer(
            "429 Too Many Requests",
            "retry-after: 18446744073709551615\r\n",
            "{}",
        ),
        false,
    )
    .await;
    let client = Client::new(Some(&s.ca), Limits::default()).unwrap();
    let resp = client.request(get(&s.url)).await.unwrap();
    assert_eq!(resp.parts.headers[RETRY_AFTER], "2592000");
    assert_eq!(client.take_wait(), Some(MAX_RETRY_AFTER));
    assert_eq!(client.take_wait(), None);
}

#[tokio::test]
async fn a_503_records_its_retry_after() {
    let s = serve(
        answer("503 Service Unavailable", "retry-after: 120\r\n", ""),
        false,
    )
    .await;
    let client = Client::new(Some(&s.ca), Limits::default()).unwrap();
    client.request(get(&s.url)).await.unwrap();
    assert_eq!(client.take_wait(), Some(Duration::from_secs(120)));
}

#[tokio::test]
async fn a_refused_nonce_is_counted_and_another_refusal_is_not() {
    let ask = |status: &'static str, kind: &'static str| async move {
        let body = format!(r#"{{"type": "urn:ietf:params:acme:error:{kind}", "detail": "no"}}"#);
        let s = serve(answer(status, "", &body), false).await;
        let client = Client::new(Some(&s.ca), Limits::default()).unwrap();
        let before = nonce_refusals();
        let mut resp = client.request(get(&s.url)).await.unwrap();
        let body = resp.body.into_bytes().await.unwrap();
        assert!(
            String::from_utf8_lossy(&body).contains(kind),
            "the body is kept"
        );
        nonce_refusals() - before
    };
    assert_eq!(ask("400 Bad Request", "malformed").await, 0);
    assert_eq!(ask("403 Forbidden", "badNonce").await, 0);
    assert_eq!(ask("400 Bad Request", "badNonce").await, 1);
}

#[test]
fn retry_after_dates_become_seconds_and_garbage_is_dropped() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let mut h = HeaderMap::new();
    h.insert(
        RETRY_AFTER,
        HeaderValue::from_static("Tue, 14 Nov 2023 22:15:20 GMT"),
    );
    assert_eq!(
        clamp_retry_after(&mut h, now),
        Some(Duration::from_secs(120))
    );
    assert_eq!(h[RETRY_AFTER], "120");
    h.insert(
        RETRY_AFTER,
        HeaderValue::from_static("Fri, 31 Dec 9999 23:59:59 GMT"),
    );
    assert_eq!(clamp_retry_after(&mut h, now), Some(MAX_RETRY_AFTER));
    h.insert(RETRY_AFTER, HeaderValue::from_static("-5"));
    assert_eq!(clamp_retry_after(&mut h, now), None);
    assert!(h.get(RETRY_AFTER).is_none());
}

#[tokio::test]
async fn a_body_over_the_cap_is_refused() {
    let big = "x".repeat(2048);
    let s = serve(answer("200 OK", "", &big), false).await;
    let limits = Limits {
        body: 1024,
        ..Limits::default()
    };
    let client = Client::new(Some(&s.ca), limits).unwrap();
    let e = client.request(get(&s.url)).await.err().unwrap();
    assert!(e.to_string().contains("reading the answer"), "{e}");
}

#[tokio::test]
async fn a_server_that_never_answers_times_out() {
    let s = serve(answer("200 OK", "", ""), true).await;
    let limits = Limits {
        request: Duration::from_millis(500),
        ..Limits::default()
    };
    let client = Client::new(Some(&s.ca), limits).unwrap();
    let e = client.request(get(&s.url)).await.err().unwrap();
    assert!(e.to_string().contains("did not answer in time"), "{e}");
}

#[tokio::test]
async fn plain_http_and_an_unknown_ca_are_refused() {
    let client = Client::new(None, Limits::default()).unwrap();
    let e = client
        .request(get("http://localhost:1/dir"))
        .await
        .err()
        .unwrap();
    assert!(e.to_string().contains("only https"), "{e}");
    let s = serve(answer("200 OK", "", ""), false).await;
    assert!(client.request(get(&s.url)).await.is_err());
}
