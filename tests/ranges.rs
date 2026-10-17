mod common;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use burrow::config::model::Protocol;
use burrow::config::settings::{Forward, Service};
use burrow::protocol::VisitTarget;
use burrow::secret::Secret;
use common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};

fn free_range(count: u16) -> u16 {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let seed = std::process::id().wrapping_mul(2_654_435_761);
    for _ in 0..200 {
        let n = NEXT.fetch_add(u32::from(count) + 1, Ordering::Relaxed);
        let base = 20000 + (seed.wrapping_add(n) % 40000) as u16;
        let all_free = (0..count).all(|i| {
            let addr = SocketAddr::from(([127, 0, 0, 1], base + i));
            std::net::TcpListener::bind(addr).is_ok() && std::net::UdpSocket::bind(addr).is_ok()
        });
        if all_free {
            return base;
        }
    }
    panic!("no free range of {count} ports");
}

async fn tagged_tcp_targets(count: u16) -> u16 {
    let base = free_range(count);
    for i in 0..count {
        let l = TcpListener::bind(("127.0.0.1", base + i)).await.unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else {
                    return;
                };
                let port = s.local_addr().unwrap().port();
                tokio::spawn(async move {
                    s.write_all(&port.to_be_bytes()).await.unwrap();
                    let (mut r, mut w) = s.split();
                    let _ = tokio::io::copy(&mut r, &mut w).await;
                    let _ = w.shutdown().await;
                });
            }
        });
    }
    base
}

async fn tagged_udp_targets(count: u16) -> u16 {
    let base = free_range(count);
    for i in 0..count {
        let s = UdpSocket::bind(("127.0.0.1", base + i)).await.unwrap();
        tokio::spawn(async move {
            let port = s.local_addr().unwrap().port();
            let mut buf = [0u8; 2048];
            while let Ok((n, from)) = s.recv_from(&mut buf).await {
                let mut reply = port.to_be_bytes().to_vec();
                reply.extend_from_slice(&buf[..n]);
                let _ = s.send_to(&reply, from).await;
            }
        });
    }
    base
}

async fn tcp_lands_on(addr: SocketAddr) -> Option<u16> {
    let mut s = TcpStream::connect(addr).await.ok()?;
    let mut port = [0u8; 2];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut port))
        .await
        .ok()?
        .ok()?;
    s.write_all(b"hello").await.ok()?;
    let mut back = [0u8; 5];
    s.read_exact(&mut back).await.ok()?;
    assert_eq!(&back, b"hello");
    Some(u16::from_be_bytes(port))
}

async fn udp_lands_on(addr: SocketAddr) -> Option<u16> {
    let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    s.connect(addr).await.unwrap();
    let mut buf = [0u8; 64];
    for _ in 0..30 {
        s.send(b"hello").await.unwrap();
        if let Ok(Ok(n)) = tokio::time::timeout(Duration::from_millis(150), s.recv(&mut buf)).await
            && n == 7
            && &buf[2..7] == b"hello"
        {
            return Some(u16::from_be_bytes([buf[0], buf[1]]));
        }
    }
    None
}

fn ranged(protocol: Protocol, bind: Option<u16>, target: u16, ports: u16) -> Service {
    Service {
        clients: vec!["home".into()],
        bind: bind.map(|p| SocketAddr::from(([127, 0, 0, 1], p))),
        target: format!("127.0.0.1:{target}"),
        ports,
        protocol,
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_public_tcp_port_reaches_its_own_target_port() {
    let target = tagged_tcp_targets(5).await;
    let public = free_range(5);
    let _t = tunnel(vec![(
        "game",
        ranged(Protocol::Tcp, Some(public), target, 5),
    )])
    .await;
    for i in 0..5 {
        let at = SocketAddr::from(([127, 0, 0, 1], public + i));
        assert_eq!(
            tcp_lands_on(at).await,
            Some(target + i),
            "public port {}",
            public + i
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_public_udp_port_reaches_its_own_target_port() {
    let target = tagged_udp_targets(3).await;
    let public = free_range(3);
    let _t = tunnel(vec![(
        "game",
        ranged(Protocol::Udp, Some(public), target, 3),
    )])
    .await;
    for i in 0..3 {
        let at = SocketAddr::from(([127, 0, 0, 1], public + i));
        assert_eq!(
            udp_lands_on(at).await,
            Some(target + i),
            "public port {}",
            public + i
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reload_resizes_a_range() {
    let target = tagged_tcp_targets(4).await;
    let public = free_range(4);
    let secret = Secret::generate();
    let settings = server_settings(vec![secret.clone()]);
    let t = tunnel_with(
        settings.clone(),
        vec![("game", ranged(Protocol::Tcp, Some(public), target, 2))],
        |c| c,
        secret,
    )
    .await;
    let port = |i: u16| SocketAddr::from(([127, 0, 0, 1], public + i));
    assert_eq!(tcp_lands_on(port(1)).await, Some(target + 1));
    assert_eq!(tcp_lands_on(port(3)).await, None);
    let mut next = settings;
    next.services.insert(
        "game".into(),
        ranged(Protocol::Tcp, Some(public), target, 4),
    );
    t.server.reload(next).await;
    for _ in 0..50 {
        if tcp_lands_on(port(3)).await == Some(target + 3) {
            assert_eq!(tcp_lands_on(port(0)).await, Some(target));
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the grown range must listen on its new ports");
}

struct Visits {
    _tunnel: Tunnel,
    visitor_secret: Secret,
    server: SocketAddr,
}

async fn visits(target: u16, ports: u16) -> Visits {
    let secret = Secret::generate();
    let visitor_secret = Secret::generate();
    let mut server = server_settings(vec![secret.clone()]);
    server.visitors.insert(
        "laptop".into(),
        visitor_auth(visitor_secret.clone(), &["home"]),
    );
    server.services.insert(
        "game".into(),
        Service {
            visitors: vec!["laptop".into()],
            ..ranged(Protocol::Tcp, None, target, ports)
        },
    );
    let tunnel = tunnel_with(
        server,
        vec![],
        |mut c| {
            c.allow = burrow::allowlist::AllowList::All;
            c
        },
        secret,
    )
    .await;
    Visits {
        server: tunnel.server.addr,
        _tunnel: tunnel,
        visitor_secret,
    }
}

impl Visits {
    fn forward(&self, local: u16, ports: u16, to: VisitTarget) -> RunningVisitor {
        let mut settings = visitor_settings(self.server, self.visitor_secret.clone(), vec![]);
        settings.forwards.insert(
            "game".into(),
            Forward {
                local: SocketAddr::from(([127, 0, 0, 1], local)),
                ports,
                to,
            },
        );
        start_visitor(settings)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_visitor_range_reaches_each_port_of_a_private_range() {
    let target = tagged_tcp_targets(3).await;
    let v = visits(target, 3).await;
    let local = free_range(5);
    let _visitor = v.forward(local, 5, VisitTarget::service("game"));
    let at = |i: u16| SocketAddr::from(([127, 0, 0, 1], local + i));
    for i in 0..3 {
        assert_eq!(
            tcp_lands_on(at(i)).await,
            Some(target + i),
            "local port {}",
            local + i
        );
    }
    for i in 3..5 {
        assert_eq!(
            tcp_lands_on(at(i)).await,
            None,
            "past the end of the service's range"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_visitor_range_reaches_a_client_port_range_directly() {
    let target = tagged_tcp_targets(3).await;
    let v = visits(target, 1).await;
    let local = free_range(3);
    let _visitor = v.forward(
        local,
        3,
        VisitTarget::Client {
            client: "home".into(),
            target: format!("127.0.0.1:{target}"),
        },
    );
    for i in 0..3 {
        let at = SocketAddr::from(([127, 0, 0, 1], local + i));
        assert_eq!(tcp_lands_on(at).await, Some(target + i));
    }
}
