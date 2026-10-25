use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::common::*;
use crate::kit::*;

const HEADERS: u8 = 1;
const END_HEADERS: u8 = 0x4;
const STATUS_200: u8 = 0x88;

#[derive(Default)]
struct Seen {
    requests: AtomicUsize,
    pings: AtomicUsize,
}

async fn h2_app_that_keeps_its_stream_open() -> (SocketAddr, Arc<Seen>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let seen = Arc::new(Seen::default());
    let s = seen.clone();
    tokio::spawn(async move {
        let Ok((tcp, _)) = l.accept().await else {
            return;
        };
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
                    frame(HEADERS, END_HEADERS, f.stream, &[STATUS_200])
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
    });
    (addr, seen)
}

#[tokio::test]
async fn an_h2_app_gets_no_keepalive_pings_during_a_long_stream() {
    let (app, seen) = h2_app_that_keeps_its_stream_open().await;
    let (up, _) = Fake::new(4);
    up.serve("grpc", app);
    let mut site = https_site("grpc", &[HOST]);
    site.options.target_http2 = true;
    let p = start_with(|s| s.sites = vec![site], up, fixed(&[HOST]), false).await;
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
