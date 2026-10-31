mod common;

use std::net::{IpAddr, SocketAddr};

use common::*;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;

const V1: &[u8] = b"PROXY TCP4 203.0.113.9 10.0.0.1 40000 443\r\n";

async fn balanced(passthrough: bool) -> (burrow_web::Proxy<Fake>, std::sync::Arc<Fake>) {
    let app = http_backend().await;
    let (up, _) = Fake::new(4);
    up.serve("blog", app.addr);
    up.serve("nas", app.addr);
    let proxy = start_with(
        move |s| {
            s.sites = vec![https_site("blog", &["example.com"])];
            if passthrough {
                s.sites.push(site("nas", &["nas.example.com"]));
            }
            s.forwarding.proxy_protocol_from = vec!["127.0.0.1/32".parse().unwrap()];
        },
        up.clone(),
        fixed(&["example.com"]),
        true,
    )
    .await;
    (proxy, up)
}

async fn with_header(addr: SocketAddr, header: &[u8]) -> TcpStream {
    let mut tcp = TcpStream::connect(addr).await.unwrap();
    tcp.write_all(header).await.unwrap();
    tcp
}

const GET: &str = "GET / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n";

#[tokio::test]
async fn a_listed_balancer_names_the_visitor_with_a_proxy_header_on_both_ports() {
    let (proxy, up) = balanced(false).await;
    let tcp = with_header(proxy.https_addr().unwrap(), V1).await;
    let name = ServerName::try_from("example.com").unwrap();
    let mut t = connector().connect(name, tcp).await.unwrap();
    let r = exchange(&mut t, GET.as_bytes()).await;
    assert!(r.contains("x-real-ip: 203.0.113.9\n"), "{r}");
    let mut plain = with_header(proxy.http_addr().unwrap(), V1).await;
    let r = exchange(&mut plain, GET.as_bytes()).await;
    assert!(r.starts_with("HTTP/1.1 308"), "{r}");
    let seen: Vec<IpAddr> = up
        .visitors
        .lock()
        .unwrap()
        .iter()
        .map(|(_, v)| v.addr.ip())
        .collect();
    assert_eq!(seen, ["203.0.113.9".parse::<IpAddr>().unwrap()]);
}

#[tokio::test]
async fn a_listed_balancer_that_sends_no_header_is_closed() {
    let (proxy, _up) = balanced(false).await;
    assert!(closed_without_tls(proxy.https_addr().unwrap(), "example.com").await);
    let r = over_plain(proxy.http_addr().unwrap(), GET).await;
    assert!(r.is_empty(), "{r}");
}

#[tokio::test]
async fn a_passthrough_visitor_behind_a_balancer_is_the_header_source() {
    let (proxy, up) = balanced(true).await;
    let tcp = with_header(proxy.https_addr().unwrap(), V1).await;
    let name = ServerName::try_from("nas.example.com").unwrap();
    let _ = connector().connect(name, tcp).await;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while up.visitors.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let v = up.visitors.lock().unwrap()[0].clone();
    assert_eq!(v.0, "nas");
    assert_eq!(v.1.addr, "203.0.113.9:40000".parse::<SocketAddr>().unwrap());
}

#[tokio::test]
async fn one_visitor_behind_a_balancer_cannot_hold_every_slot_with_stalled_connections() {
    let app = http_backend().await;
    let (up, _) = Fake::new(4);
    up.serve("blog", app.addr);
    let proxy = start_with(
        |s| {
            s.sites = vec![https_site("blog", &["example.com"])];
            s.forwarding.proxy_protocol_from = vec!["127.0.0.1/32".parse().unwrap()];
            s.max_pending = 16;
        },
        up,
        fixed(&["example.com"]),
        false,
    )
    .await;
    let addr = proxy.https_addr().unwrap();
    let mut held = Vec::new();
    for _ in 0..24 {
        held.push(with_header(addr, V1).await);
    }
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(proxy.refused(), 0, "nothing is refused before it stalls");
    let started = std::time::Instant::now();
    let name = ServerName::try_from("example.com").unwrap();
    let other = with_header(addr, b"PROXY TCP4 198.51.100.20 10.0.0.1 40001 443\r\n").await;
    let mut t = connector().connect(name, other).await.unwrap();
    let r = exchange(&mut t, GET.as_bytes()).await;
    assert!(r.contains("x-real-ip: 198.51.100.20\n"), "{r}");
    assert!(
        started.elapsed() < std::time::Duration::from_millis(2500),
        "served soon after the stalled ones are cut, well before the deadline: {:?}",
        started.elapsed()
    );
    wait_until(
        "all but an eighth (at least 8) of one visitor's stalled connections are closed",
        || proxy.refused() >= 16,
    )
    .await;
    drop(held);
}
