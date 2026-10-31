mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use burrow_web::http::health::{Ask, HealthCheck};
use burrow_web::{BoxIo, SiteOptions, TargetTls, pipe};
use common::*;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};

const AGENT: &str = "burrow-health-check/9.9.9";

type Heard = Arc<Mutex<Vec<String>>>;

async fn head_of<S: AsyncRead + Unpin>(s: &mut S) -> String {
    let mut got = Vec::new();
    let mut one = [0u8; 1];
    while !got.ends_with(b"\r\n\r\n") {
        match s.read(&mut one).await {
            Ok(1) => got.push(one[0]),
            _ => break,
        }
    }
    String::from_utf8_lossy(&got).into_owned()
}

async fn answer<S: AsyncRead + AsyncWrite + Unpin>(mut s: S, reply: &'static str, heard: Heard) {
    let head = head_of(&mut s).await;
    heard.lock().unwrap().push(head);
    let _ = s.write_all(reply.as_bytes()).await;
    let _ = s.flush().await;
    std::future::pending::<()>().await;
}

fn app(reply: &'static str) -> (BoxIo, Heard) {
    let (ours, theirs) = pipe();
    let heard = Heard::default();
    tokio::spawn(answer(theirs, reply, heard.clone()));
    (Box::new(ours), heard)
}

fn tls_app(
    cert: (CertificateDer<'static>, PrivateKeyDer<'static>),
    reply: &'static str,
) -> (BoxIo, Heard) {
    let config = ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert.0], cert.1)
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let (ours, theirs) = pipe();
    let heard = Heard::default();
    let log = heard.clone();
    tokio::spawn(async move {
        if let Ok(tls) = acceptor.accept(theirs).await {
            answer(tls, reply, log).await;
        }
    });
    (Box::new(ours), heard)
}

fn checked(host: &str, options: SiteOptions, path: &str) -> HealthCheck {
    let ask = Ask {
        host,
        path,
        agent: AGENT,
    };
    HealthCheck::build(&options, &ask).unwrap()
}

fn plain(path: &str) -> HealthCheck {
    checked("blog.example.com", SiteOptions::default(), path)
}

async fn run(check: &HealthCheck, io: BoxIo) -> Result<(), String> {
    tokio::time::timeout(Duration::from_secs(5), check.run(io))
        .await
        .expect("the check ends")
}

const OK: &str = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";

#[tokio::test]
async fn the_check_asks_for_the_path_with_the_sites_first_name_and_its_own_agent() {
    let (io, heard) = app(OK);
    assert_eq!(run(&plain("/healthz?full=1"), io).await, Ok(()));
    let head = heard.lock().unwrap()[0].to_ascii_lowercase();
    assert!(
        head.starts_with("get /healthz?full=1 http/1.1\r\n"),
        "{head}"
    );
    assert!(head.contains("\r\nhost: blog.example.com\r\n"), "{head}");
    assert!(
        head.contains(&format!("\r\nuser-agent: {AGENT}\r\n")),
        "{head}"
    );
    assert!(head.contains("\r\naccept: */*\r\n"), "{head}");
    assert!(head.contains("\r\nconnection: close\r\n"), "{head}");
    for never in ["x-forwarded", "x-real-ip", "authorization", "forwarded"] {
        assert!(!head.contains(never), "{never} in {head}");
    }
}

#[tokio::test]
async fn a_redirect_is_up_and_not_followed() {
    let (io, heard) = app(
        "HTTP/1.1 302 Found\r\nLocation: http://elsewhere.example.com/login\r\nContent-Length: 0\r\n\r\n",
    );
    assert_eq!(run(&plain("/"), io).await, Ok(()));
    assert_eq!(heard.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_login_or_an_error_is_down_with_the_answer_as_the_reason() {
    for (reply, why) in [
        (
            "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n",
            "answered 401 Unauthorized",
        ),
        (
            "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n",
            "answered 403 Forbidden",
        ),
        (
            "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n",
            "answered 400 Bad Request",
        ),
        (
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n",
            "answered 500 Internal Server Error",
        ),
    ] {
        let (io, _) = app(reply);
        assert_eq!(run(&plain("/"), io).await, Err(why.to_string()));
    }
}

#[tokio::test]
async fn an_app_that_is_not_http_is_down() {
    let (io, _) = app("SSH-2.0-OpenSSH_9.6\r\n\r\n");
    let why = run(&plain("/"), io).await.unwrap_err();
    assert!(why.starts_with("the app did not answer HTTP"), "{why}");
    let (ours, theirs) = pipe();
    drop(theirs);
    let why = run(&plain("/"), Box::new(ours)).await.unwrap_err();
    assert!(why.contains("the app"), "{why}");
}

#[tokio::test]
async fn a_slow_body_does_not_hold_the_check_once_the_head_is_in() {
    let (io, _) = app("HTTP/1.1 200 OK\r\nContent-Length: 1000000\r\n\r\nstart");
    assert_eq!(run(&plain("/"), io).await, Ok(()));
}

#[tokio::test]
async fn an_app_that_never_answers_keeps_the_check_waiting_for_its_timeout() {
    let (ours, _theirs) = pipe();
    let check = plain("/");
    let waited = tokio::time::timeout(Duration::from_millis(300), check.run(Box::new(ours))).await;
    assert!(waited.is_err());
}

#[tokio::test]
async fn a_tls_target_is_checked_with_its_ca_and_a_wrong_ca_is_down_with_the_tls_reason() {
    let dir = tempfile::tempdir().unwrap();
    let (ours, other) = (ca(), ca());
    let (ours_pem, other_pem) = (dir.path().join("ours.pem"), dir.path().join("other.pem"));
    std::fs::write(&ours_pem, &ours.pem).unwrap();
    std::fs::write(&other_pem, &other.pem).unwrap();
    let options = |target_tls| SiteOptions {
        target_tls,
        target_name: "erp.lan".into(),
        ..SiteOptions::default()
    };
    let (io, heard) = tls_app(leaf(Some(&ours), &["erp.lan"]), OK);
    let check = checked("erp.example.com", options(TargetTls::Ca(ours_pem)), "/up");
    assert_eq!(run(&check, io).await, Ok(()));
    assert!(heard.lock().unwrap()[0].contains("\r\nhost: erp.example.com\r\n"));
    let (io, heard) = tls_app(leaf(Some(&ours), &["erp.lan"]), OK);
    let check = checked("erp.example.com", options(TargetTls::Ca(other_pem)), "/up");
    let why = run(&check, io).await.unwrap_err();
    assert!(why.starts_with("TLS to the app failed: "), "{why}");
    assert!(why.contains("invalid peer certificate"), "{why}");
    assert!(heard.lock().unwrap().is_empty());
    let missing = options(TargetTls::Ca(dir.path().join("missing.pem")));
    let (io, _) = tls_app(leaf(Some(&ours), &["erp.lan"]), OK);
    let why = run(&checked("erp.example.com", missing, "/up"), io)
        .await
        .unwrap_err();
    assert!(why.starts_with("target_tls: cannot read "), "{why}");
}

#[tokio::test]
async fn an_unchecked_tls_target_passes_with_a_self_signed_certificate() {
    let options = SiteOptions {
        target_tls: TargetTls::Unchecked,
        target_name: "erp.lan".into(),
        ..SiteOptions::default()
    };
    let (io, _) = tls_app(leaf(None, &["erp.lan"]), OK);
    assert_eq!(
        run(&checked("erp.example.com", options, "/"), io).await,
        Ok(())
    );
}

type Seen = Arc<Mutex<Vec<(http::Version, String, String)>>>;

async fn h2_answer<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(io: S, seen: Seen) {
    let service = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
        let seen = seen.clone();
        async move {
            let ua = req
                .headers()
                .get("user-agent")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            seen.lock()
                .unwrap()
                .push((req.version(), req.uri().to_string(), ua));
            let mut r = hyper::Response::new(http_body_util::Empty::<bytes::Bytes>::new());
            *r.status_mut() = http::StatusCode::NO_CONTENT;
            Ok::<_, std::convert::Infallible>(r)
        }
    });
    let _ = hyper::server::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::new())
        .serve_connection(hyper_util::rt::TokioIo::new(io), service)
        .await;
}

fn h2_options(target_tls: TargetTls) -> SiteOptions {
    SiteOptions {
        target_tls,
        target_name: "grpc.lan".into(),
        target_http2: true,
        ..SiteOptions::default()
    }
}

fn h2_tls_app(alpn: &[&[u8]]) -> (BoxIo, Seen) {
    let cert = leaf(None, &["grpc.lan"]);
    let mut config = ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert.0], cert.1)
        .unwrap();
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let (ours, theirs) = pipe();
    let seen = Seen::default();
    let log = seen.clone();
    tokio::spawn(async move {
        if let Ok(tls) = acceptor.accept(theirs).await {
            h2_answer(tls, log).await;
        }
    });
    (Box::new(ours), seen)
}

#[tokio::test]
async fn a_target_http2_app_without_tls_is_checked_over_h2c() {
    let (ours, theirs) = pipe();
    let seen = Seen::default();
    tokio::spawn(h2_answer(theirs, seen.clone()));
    let check = checked("grpc.example.com", h2_options(TargetTls::Off), "/healthz");
    assert_eq!(run(&check, Box::new(ours)).await, Ok(()));
    let (version, uri, ua) = seen.lock().unwrap()[0].clone();
    assert_eq!(version, http::Version::HTTP_2);
    assert_eq!(uri, "http://grpc.example.com/healthz");
    assert_eq!(ua, AGENT);
}

#[tokio::test]
async fn a_target_http2_app_over_tls_is_checked_over_h2_and_must_agree_to_it() {
    let (io, seen) = h2_tls_app(&[b"h2"]);
    let check = checked(
        "grpc.example.com",
        h2_options(TargetTls::Unchecked),
        "/ready",
    );
    assert_eq!(run(&check, io).await, Ok(()));
    let (version, uri, _) = seen.lock().unwrap()[0].clone();
    assert_eq!(version, http::Version::HTTP_2);
    assert_eq!(uri, "https://grpc.example.com/ready");
    let (io, seen) = h2_tls_app(&[]);
    let why = run(&check, io).await.unwrap_err();
    assert!(why.contains("did not agree to HTTP/2"), "{why}");
    assert!(seen.lock().unwrap().is_empty());
}
