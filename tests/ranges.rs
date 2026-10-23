mod common;

use std::net::SocketAddr;
use std::time::Duration;

use burrow::config::settings::{Forward, Service};
use burrow::protocol::VisitTarget;
use burrow::secret::Secret;
use burrow_config::Protocol;
use common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};

use common::free_range;

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
    let s = udp_bind().await;
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
                socks: false,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_udp_range_does_not_use_up_the_clients_connection_limit() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let mut server = server_settings(vec![secret.clone()]);
    server.clients.get_mut("home").unwrap().max_connections = Some(4);
    let base = {
        let mut found = None;
        for p in (41000u16..60000).step_by(7) {
            if (0..4).all(|i| std::net::UdpSocket::bind(("127.0.0.1", p + i)).is_ok()) {
                found = Some(p);
                break;
            }
        }
        found.unwrap()
    };
    let t = tunnel_with(
        server,
        vec![
            (
                "voice",
                Service {
                    clients: vec!["home".into()],
                    bind: Some(SocketAddr::from(([127, 0, 0, 1], base))),
                    target: "127.0.0.1:9".into(),
                    ports: 4,
                    protocol: Protocol::Udp,
                    ..Default::default()
                },
            ),
            ("web", service(echo, Protocol::Tcp)),
        ],
        |c| c,
        secret,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let web = t.server.services["web"];
    assert_eq!(
        try_roundtrip(web, b"ping").await.as_deref(),
        Some(&b"ping"[..]),
        "four idle UDP ports took all four connection slots of client \"home\""
    );
}

fn free_tcp_range(from: u16, count: u16) -> u16 {
    (from..60000)
        .step_by(11)
        .find(|p| (0..count).all(|i| std::net::TcpListener::bind(("127.0.0.1", p + i)).is_ok()))
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shrinking_a_range_closes_connections_on_the_ports_it_dropped() {
    use tokio::io::AsyncWriteExt;
    let target = free_tcp_range(42000, 2);
    for i in 0..2 {
        let l = tokio::net::TcpListener::bind(("127.0.0.1", target + i))
            .await
            .unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                tokio::spawn(async move {
                    let (mut r, mut w) = s.split();
                    let _ = tokio::io::copy(&mut r, &mut w).await;
                    let _ = w.shutdown().await;
                });
            }
        });
    }
    let public = free_tcp_range(target + 100, 2);
    let ranged = |ports: u16| Service {
        clients: vec!["home".into()],
        bind: Some(SocketAddr::from(([127, 0, 0, 1], public))),
        target: format!("127.0.0.1:{target}"),
        ports,
        ..Default::default()
    };
    let secret = Secret::generate();
    let settings = server_settings(vec![secret.clone()]);
    let t = tunnel_with(settings.clone(), vec![("game", ranged(2))], |c| c, secret).await;
    let conn = open_echo(SocketAddr::from(([127, 0, 0, 1], public + 1))).await;
    let mut next = settings;
    next.services.insert("game".into(), ranged(1));
    t.server.reload(next).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", public + 1))
            .await
            .is_err(),
        "the dropped port no longer listens"
    );
    let mut conn = conn;
    let _ = conn.flush().await;
    assert_cut(conn).await;
}

async fn udp_visits(target: u16, ports: u16) -> Visits {
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
            ..ranged(Protocol::Udp, None, target, ports)
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
    async fn udp_lands_on(&self, to: VisitTarget) -> Result<u16, Option<burrow::protocol::Status>> {
        let mut v = udp_visit(self.server, self.visitor_secret.clone(), to).await?;
        for _ in 0..10 {
            v.send(9, b"hello").await;
            match v.next(Duration::from_millis(300)).await {
                Got::Datagram(9, b) if b.len() == 7 && &b[2..] == b"hello" => {
                    return Ok(u16::from_be_bytes([b[0], b[1]]));
                }
                Got::Closed => return Err(None),
                _ => {}
            }
        }
        Err(None)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_udp_visit_reaches_each_port_of_a_private_udp_range() {
    let target = tagged_udp_targets(3).await;
    let v = udp_visits(target, 3).await;
    for i in 0..3 {
        assert_eq!(
            v.udp_lands_on(VisitTarget::service("game").at(i)).await,
            Ok(target + i)
        );
    }
    assert_eq!(
        v.udp_lands_on(VisitTarget::service("game").at(3)).await,
        Err(Some(burrow::protocol::Status::Invalid))
    );
    let direct = VisitTarget::Client {
        client: "home".into(),
        target: format!("127.0.0.1:{target}"),
    };
    for i in 0..3 {
        assert_eq!(v.udp_lands_on(direct.at(i)).await, Ok(target + i));
    }
}
