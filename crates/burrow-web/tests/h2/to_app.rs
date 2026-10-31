use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::common::*;
use crate::kit::*;

const HEADERS: u8 = 1;
const RST_STREAM: u8 = 3;
const END_HEADERS: u8 = 0x4;
const STATUS_200: u8 = 0x88;
const CANCEL: u32 = 0x8;

#[derive(Clone, Copy)]
enum Answer {
    HeadThenQuiet,
    Cancel,
}

#[derive(Default)]
struct Seen {
    requests: AtomicUsize,
    pings: AtomicUsize,
}

async fn h2_app(answer: Answer) -> (SocketAddr, Arc<Seen>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let seen = Arc::new(Seen::default());
    let s = seen.clone();
    tokio::spawn(async move {
        while let Ok((tcp, _)) = l.accept().await {
            tokio::spawn(serve_one(tcp, answer, s.clone()));
        }
    });
    (addr, seen)
}

async fn serve_one(tcp: tokio::net::TcpStream, answer: Answer, s: Arc<Seen>) {
    let (mut rd, mut wr) = tcp.into_split();
    let mut preface = [0u8; 24];
    if rd.read_exact(&mut preface).await.is_err() || preface != PREFACE {
        return;
    }
    let _ = wr.write_all(&frame(SETTINGS, 0, 0, &[])).await;
    while let Some(f) = read_frame(&mut rd, Duration::from_secs(120)).await {
        let reply = match f.ty {
            SETTINGS if f.flags & ACK == 0 => frame(SETTINGS, ACK, 0, &[]),
            HEADERS => {
                s.requests.fetch_add(1, Ordering::SeqCst);
                match answer {
                    Answer::HeadThenQuiet => frame(HEADERS, END_HEADERS, f.stream, &[STATUS_200]),
                    Answer::Cancel => frame(RST_STREAM, 0, f.stream, &CANCEL.to_be_bytes()),
                }
            }
            PING if f.flags & ACK == 0 => {
                s.pings.fetch_add(1, Ordering::SeqCst);
                frame(PING, ACK, 0, &f.payload)
            }
            _ => continue,
        };
        if wr.write_all(&reply).await.is_err() {
            return;
        }
    }
}

async fn h2_site(up: Arc<Fake>) -> burrow_web::Proxy<Fake> {
    let mut site = https_site("grpc", &[HOST]);
    site.options.target_http2 = true;
    start_with(|s| s.sites = vec![site], up, fixed(&[HOST]), false).await
}

#[tokio::test]
async fn an_h2_app_gets_no_keepalive_pings_during_a_long_stream() {
    let (app, seen) = h2_app(Answer::HeadThenQuiet).await;
    let (up, _) = Fake::new(4);
    up.serve("grpc", app);
    let p = h2_site(up).await;
    let c = client(p.https_addr().unwrap()).await;
    let mut c = c.ready().await.unwrap();
    let (resp, _) = c
        .send_request(request(&site_uri("/long"), &[]), true)
        .unwrap();
    let resp = tokio::time::timeout(Duration::from_secs(10), resp)
        .await
        .expect("the app's head in time")
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.into_body();
    tokio::time::sleep(Duration::from_secs(35)).await;
    assert_eq!(seen.requests.load(Ordering::SeqCst), 1);
    assert_eq!(
        seen.pings.load(Ordering::SeqCst),
        0,
        "grpc-go closes a connection that pings more often than every 5 minutes (keepalive.EnforcementPolicy MinTime)"
    );
    drop(body);
}

#[tokio::test]
async fn an_app_reset_before_its_head_reaches_an_h2_visitor_as_the_same_reset() {
    let (app, seen) = h2_app(Answer::Cancel).await;
    let (up, _) = Fake::new(4);
    up.serve("grpc", app);
    let p = h2_site(up).await;
    let addr = p.https_addr().unwrap();
    let c = client(addr).await;
    let mut c = c.ready().await.unwrap();
    let (resp, _) = c
        .send_request(request(&site_uri("/pkg.Svc/Call"), &[]), true)
        .unwrap();
    let err = tokio::time::timeout(Duration::from_secs(10), resp)
        .await
        .expect("an answer in time")
        .expect_err("a reset, not a 502 page");
    assert_eq!(err.reason(), Some(h2::Reason::CANCEL), "{err:?}");
    let mut t = tls_with(addr, HOST, &[H1]).await.unwrap();
    let r = exchange(
        &mut t,
        format!("GET / HTTP/1.1\r\nHost: {HOST}\r\nConnection: close\r\n\r\n").as_bytes(),
    )
    .await;
    assert!(
        r.starts_with("HTTP/1.1 502"),
        "an HTTP/1.1 visitor has no stream to reset: {r}"
    );
    assert_eq!(seen.requests.load(Ordering::SeqCst), 2);
}
