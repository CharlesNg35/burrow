mod common;

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;

const GET: &str = "GET / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n";

async fn balanced(pool: usize, handshake: Duration) -> burrow_web::Proxy<Fake> {
    let app = http_backend().await;
    let (up, _) = Fake::new(4);
    up.serve("blog", app.addr);
    start_with(
        move |s| {
            s.sites = vec![https_site("blog", &["example.com"])];
            s.forwarding.proxy_protocol_from = vec!["127.0.0.1/32".parse().unwrap()];
            s.max_pending = pool;
            s.timeouts.handshake = handshake;
        },
        up,
        fixed(&["example.com"]),
        false,
    )
    .await
}

fn header(from: &str) -> Vec<u8> {
    format!("PROXY TCP4 {from} 10.0.0.1 40000 443\r\n").into_bytes()
}

async fn from(addr: SocketAddr, who: &str) -> TcpStream {
    let mut tcp = TcpStream::connect(addr).await.unwrap();
    tcp.write_all(&header(who)).await.unwrap();
    tcp
}

async fn visit(addr: SocketAddr, who: &str) -> String {
    let tcp = from(addr, who).await;
    let name = ServerName::try_from("example.com").unwrap();
    let mut t = connector().connect(name, tcp).await.expect("TLS");
    exchange(&mut t, GET.as_bytes()).await
}

async fn closed(s: &mut TcpStream) -> bool {
    let mut buf = [0u8; 1];
    let ended = tokio::time::timeout(Duration::from_secs(1), s.read(&mut buf)).await;
    matches!(ended, Ok(Ok(0)) | Ok(Err(_)))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_burst_of_three_pools_from_one_address_is_served_in_full() {
    let proxy = balanced(4, Duration::from_secs(5)).await;
    let addr = proxy.https_addr().unwrap();
    let burst: Vec<_> = (0..12)
        .map(|_| tokio::spawn(async move { visit(addr, "203.0.113.7").await }))
        .collect();
    for v in burst {
        let r = v.await.unwrap();
        assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    }
    assert_eq!(proxy.refused(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn many_handshakes_from_one_address_all_get_through_a_small_pool() {
    let proxy = balanced(8, Duration::from_secs(10)).await;
    let addr = proxy.https_addr().unwrap();
    for _ in 0..15 {
        let wave: Vec<_> = (0..20)
            .map(|_| tokio::spawn(async move { visit(addr, "203.0.113.7").await }))
            .collect();
        for v in wave {
            let r = v.await.unwrap();
            assert!(r.starts_with("HTTP/1.1 200"), "{r}");
        }
    }
    assert_eq!(proxy.refused(), 0, "nothing is dropped, everything waits");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn silent_sockets_from_one_address_cannot_keep_another_one_waiting() {
    let proxy = balanced(16, Duration::from_secs(5)).await;
    let addr = proxy.https_addr().unwrap();
    let mut flood = Vec::new();
    for _ in 0..16 * 5 {
        flood.push(from(addr, "203.0.113.7").await);
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        proxy.admitted() <= 16 + 33,
        "one address fills at most its share of the waiting room: {}",
        proxy.admitted()
    );
    let started = Instant::now();
    let r = visit(addr, "198.51.100.20").await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    assert!(
        started.elapsed() < Duration::from_millis(2500),
        "{:?}",
        started.elapsed()
    );
    tokio::time::sleep(Duration::from_secs(5)).await;
    for s in &mut flood {
        assert!(
            closed(s).await,
            "every silent socket is closed by its deadline"
        );
    }
    assert_eq!(proxy.admitted(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_waiting_room_stops_accepting_and_the_backlog_is_served_later() {
    let proxy = balanced(2, Duration::from_secs(1)).await;
    let addr = proxy.https_addr().unwrap();
    let mut flood = Vec::new();
    for _ in 0..20 {
        flood.push(from(addr, "127.0.0.1").await);
    }
    let late = tokio::spawn(async move { visit(addr, "198.51.100.20").await });
    let mut most = 0;
    while !late.is_finished() {
        most = most.max(proxy.admitted());
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(late.await.unwrap().starts_with("HTTP/1.1 200"));
    assert_eq!(
        most, 10,
        "the pool and four times it in the waiting room, no more"
    );
}
