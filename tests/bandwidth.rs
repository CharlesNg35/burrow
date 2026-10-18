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
    let s = udp_bind().await;
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

async fn udp_socket_to(target: SocketAddr) -> UdpSocket {
    let s = udp_bind().await;
    s.connect(target).await.unwrap();
    let mut buf = [0u8; 64];
    for _ in 0..50 {
        s.send(b"warm").await.unwrap();
        if tokio::time::timeout(Duration::from_millis(100), s.recv(&mut buf))
            .await
            .is_ok()
        {
            return s;
        }
    }
    panic!("the UDP relay never answered");
}

async fn burst_through(s: &UdpSocket, count: usize, len: usize) -> usize {
    let payload = vec![9u8; len];
    let mut buf = vec![0u8; 65536];
    while tokio::time::timeout(Duration::from_millis(50), s.recv(&mut buf))
        .await
        .is_ok()
    {}
    let send = async {
        for _ in 0..count {
            s.send(&payload).await.unwrap();
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    };
    let recv = async {
        let mut got = 0;
        while let Ok(Ok(n)) =
            tokio::time::timeout(Duration::from_millis(500), s.recv(&mut buf)).await
        {
            if n == len {
                got += 1;
            }
        }
        got
    };
    tokio::join!(send, recv).1
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removing_the_bandwidth_of_a_udp_service_lifts_the_limit() {
    let echo = udp_echo_server().await;
    let secret = Secret::generate();
    let settings = server_settings(vec![secret.clone()]);
    let dns = service(echo, Protocol::Udp);
    let limited = Service {
        bandwidth: Some(100_000),
        ..dns.clone()
    };
    let t = tunnel_with(
        settings.clone(),
        vec![("dns", limited), ("free", dns.clone())],
        |c| c,
        secret,
    )
    .await;
    let control = udp_socket_to(t.server.services["free"]).await;
    let s = udp_socket_to(t.server.services["dns"]).await;
    let mut next = settings;
    next.services.insert("dns".into(), dns.clone());
    next.services.insert("free".into(), dns);
    t.server.reload(next).await;
    let shared = t.server.shared.clone();
    wait_for(|| {
        shared.settings_snapshot().services["dns"]
            .bandwidth
            .is_none()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let free = burst_through(&control, 200, 1000).await;
    let got = burst_through(&s, 200, 1000).await;
    assert!(free >= 150, "the unlimited control passed only {free}");
    assert!(
        got >= 150,
        "after the bandwidth was removed only {got} of 200 datagrams passed (control: {free})"
    );
}

async fn echoes_once(s: &UdpSocket, payload: &[u8]) -> bool {
    let mut buf = vec![0u8; 65536];
    for _ in 0..6 {
        s.send(payload).await.unwrap();
        if let Ok(Ok(n)) = tokio::time::timeout(Duration::from_millis(400), s.recv(&mut buf)).await
            && n == payload.len()
        {
            return true;
        }
    }
    false
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_udp_datagram_larger_than_the_burst_can_pass_an_idle_limit() {
    let echo = udp_echo_server().await;
    let dns = service(echo, Protocol::Udp);
    let t = tunnel(vec![
        (
            "big",
            Service {
                bandwidth: Some(100_000),
                ..dns.clone()
            },
        ),
        ("free", dns),
    ])
    .await;
    let control = udp_socket_to(t.server.services["free"]).await;
    let s = udp_socket_to(t.server.services["big"]).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let payload = vec![3u8; 20_000];
    assert!(
        echoes_once(&control, &payload).await,
        "the unlimited control forwards it"
    );
    assert!(
        echoes_once(&s, &payload).await,
        "a 20000-byte datagram is never forwarded under a 100kb/s limit, even when the link is idle"
    );
}

#[test]
fn the_minimum_rate_named_in_the_error_is_accepted() {
    for text in ["1kb", "8kbit"] {
        let r = burrow::ratelimit::parse_rate(text);
        assert!(r.is_ok(), "{text}: {r:?}");
    }
}

#[tokio::test]
async fn a_slow_limit_reads_in_reasonable_chunks() {
    use burrow::ratelimit::{Limit, Throttled};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let limit = Limit::new(2_000);
    let (mut a, b) = tokio::io::duplex(1 << 20);
    a.write_all(&vec![1u8; 200_000]).await.unwrap();
    let mut r = Throttled::new(b, Some(&limit.inbound));
    let mut buf = vec![0u8; 65536];
    let mut total = 0;
    while total < 16 * 1024 {
        total += r.read(&mut buf).await.unwrap();
    }
    let started = Instant::now();
    let mut reads = 0usize;
    let mut bytes = 0usize;
    while started.elapsed() < Duration::from_millis(1000) {
        bytes += r.read(&mut buf).await.unwrap();
        reads += 1;
    }
    let average = bytes as f64 / reads as f64;
    assert!(
        reads <= 100,
        "{reads} reads of {average:.1} bytes on average in one second at 2000 bytes/s"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connections_opened_before_and_after_a_rate_change_still_share_the_cap() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let settings = server_settings(vec![secret.clone()]);
    let web = service(echo, Protocol::Tcp);
    let at = |rate| Service {
        bandwidth: Some(rate),
        ..web.clone()
    };
    let t = tunnel_with(settings.clone(), vec![("web", at(200_000))], |c| c, secret).await;
    let addr = t.server.services["web"];
    let started = Instant::now();
    let first = tokio::spawn(async move {
        let data = pattern(400_000, 1);
        assert_eq!(roundtrip(addr, &data).await, data);
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut next = settings;
    next.services.insert("web".into(), at(210_000));
    t.server.reload(next).await;
    let shared = t.server.shared.clone();
    wait_for(|| shared.settings_snapshot().services["web"].bandwidth == Some(210_000)).await;
    let data = pattern(400_000, 2);
    assert_eq!(roundtrip(addr, &data).await, data);
    first.await.unwrap();
    let took = started.elapsed();
    assert!(
        took >= Duration::from_millis(3000),
        "800000 bytes each way under a ~200kb/s cap finished in {took:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn services_are_unlimited_unless_bandwidth_is_set() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let settings = server_settings(vec![secret.clone()]);
    let web = service(echo, Protocol::Tcp);
    assert_eq!(web.bandwidth, None, "no bandwidth unless configured");
    let t = tunnel_with(settings.clone(), vec![("web", web.clone())], |c| c, secret).await;
    let addr = t.server.services["web"];
    assert_eq!(t.server.shared.status().services[0].bandwidth, None);
    let fast = timed_roundtrip(addr, 4_000_000, 7).await;
    assert!(
        fast < Duration::from_secs(2),
        "an unlimited service took {fast:?}"
    );

    let mut next = settings.clone();
    next.services.insert("web".into(), limited(web.clone()));
    t.server.reload(next).await;
    for _ in 0..50 {
        if t.server.shared.status().services[0].bandwidth.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let slow = timed_roundtrip(addr, 800_000, 8).await;
    assert!(
        slow >= Duration::from_millis(1200),
        "the added limit took only {slow:?}"
    );

    let mut next = settings;
    next.services.insert("web".into(), web);
    t.server.reload(next).await;
    for _ in 0..50 {
        if t.server.shared.status().services[0].bandwidth.is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let again = timed_roundtrip(addr, 4_000_000, 9).await;
    assert!(
        again < Duration::from_secs(2),
        "removing the limit left {again:?}"
    );
}
