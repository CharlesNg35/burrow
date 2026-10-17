mod common;

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use burrow::config::model::Protocol;
use burrow::config::settings::Service;
use burrow::protocol::VisitTarget;
use burrow::secret::Secret;
use common::*;
use tokio::net::UdpSocket;

const RATE: u64 = 500_000;

fn limited(base: Service) -> Service {
    Service {
        bandwidth: Some(RATE),
        ..base
    }
}

async fn timed_roundtrip(addr: SocketAddr, len: usize, seed: u64) -> Duration {
    let data = pattern(len, seed);
    let started = Instant::now();
    assert_eq!(roundtrip(addr, &data).await, data);
    started.elapsed()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tcp_service_is_held_to_its_bandwidth() {
    let echo = echo_server().await;
    let t = tunnel(vec![
        ("slow", limited(service(echo, Protocol::Tcp))),
        ("fast", service(echo, Protocol::Tcp)),
    ])
    .await;
    let fast = timed_roundtrip(t.server.services["fast"], 800_000, 1).await;
    assert!(fast < Duration::from_millis(800), "unlimited took {fast:?}");
    let slow = timed_roundtrip(t.server.services["slow"], 800_000, 2).await;
    assert!(
        slow >= Duration::from_millis(1200),
        "limited took only {slow:?}"
    );
    assert!(slow <= Duration::from_secs(6), "limited took {slow:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connections_of_a_service_share_its_bandwidth() {
    let echo = echo_server().await;
    let t = tunnel(vec![("slow", limited(service(echo, Protocol::Tcp)))]).await;
    let addr = t.server.services["slow"];
    let started = Instant::now();
    let a = tokio::spawn(timed_roundtrip(addr, 400_000, 3));
    let b = tokio::spawn(timed_roundtrip(addr, 400_000, 4));
    a.await.unwrap();
    b.await.unwrap();
    let both = started.elapsed();
    assert!(
        both >= Duration::from_millis(1200),
        "two connections took only {both:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn visits_to_a_private_service_are_limited_too() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let visitor_secret = Secret::generate();
    let mut server = server_settings(vec![secret.clone()]);
    server
        .visitors
        .insert("laptop".into(), visitor_auth(visitor_secret.clone(), &[]));
    server
        .services
        .insert("db".into(), limited(private_service(echo, &["laptop"])));
    let t = tunnel_with(server, vec![], |c| c, secret).await;
    let v = start_visitor(visitor_settings(
        t.server.addr,
        visitor_secret,
        vec![("db", VisitTarget::service("db"))],
    ));
    let took = timed_roundtrip(v.addr("db"), 800_000, 5).await;
    assert!(
        took >= Duration::from_millis(1200),
        "visit took only {took:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn udp_datagrams_over_the_rate_are_dropped() {
    let echo = udp_echo_server().await;
    let t = tunnel(vec![(
        "dns",
        Service {
            bandwidth: Some(100_000),
            ..service(echo, Protocol::Udp)
        },
    )])
    .await;
    let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    s.connect(t.server.services["dns"]).await.unwrap();
    let mut buf = [0u8; 2048];
    for _ in 0..50 {
        s.send(b"warm").await.unwrap();
        if tokio::time::timeout(Duration::from_millis(100), s.recv(&mut buf))
            .await
            .is_ok()
        {
            break;
        }
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let payload = [9u8; 1000];
    for _ in 0..200 {
        s.send(&payload).await.unwrap();
    }
    let mut got = 0;
    while let Ok(Ok(n)) = tokio::time::timeout(Duration::from_millis(500), s.recv(&mut buf)).await {
        if n == payload.len() {
            got += 1;
        }
    }
    assert!(got > 0, "some datagrams pass");
    assert!(got < 80, "{got} of 200 datagrams passed a 100kb limit");
}
