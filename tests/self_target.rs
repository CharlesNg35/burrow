mod common;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use burrow::allowlist::{AllowList, Rule};
use burrow::client::State;
use burrow::config::model::Protocol;
use burrow::config::settings::{Health, ServerSettings, Service};
use burrow::protocol::VisitTarget;
use burrow::secret::Secret;
use common::*;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream, UdpSocket};

fn rules(list: &[&str]) -> AllowList {
    AllowList::Rules(list.iter().map(|r| r.parse::<Rule>().unwrap()).collect())
}

fn on_self(target: SocketAddr, protocol: Protocol) -> Service {
    Service {
        clients: vec!["self".into()],
        ..service(target, protocol)
    }
}

fn self_settings(services: Vec<(&str, Service)>) -> ServerSettings {
    let mut s = server_settings(vec![Secret::generate()]);
    s.self_allow = Some(rules(&["127.0.0.0/8"]));
    for (name, svc) in services {
        s.services.insert(name.to_string(), svc);
    }
    s
}

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

async fn tagged_targets(count: u16) -> u16 {
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
                    let _ = s.write_all(&port.to_be_bytes()).await;
                });
            }
        });
    }
    base
}

async fn lands_on(addr: SocketAddr) -> u16 {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let mut port = [0u8; 2];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut port))
        .await
        .expect("the target answers in time")
        .unwrap();
    u16::from_be_bytes(port)
}

fn client_open(server: &RunningServer, name: &str) -> u64 {
    server
        .shared
        .status()
        .clients
        .iter()
        .find(|c| c.name == name)
        .map_or(0, |c| c.open)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_public_tcp_service_on_self_reaches_its_target_without_a_client() {
    logs();
    let echo = echo_server().await;
    let srv = start_server(self_settings(vec![("web", on_self(echo, Protocol::Tcp))])).await;
    let data = pattern(300_000, 1);
    assert_eq!(roundtrip(srv.services["web"], &data).await, data);
    assert!(srv.shared.session_names().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_self_service_without_self_allow_reaches_its_own_target() {
    logs();
    let echo = echo_server().await;
    let mut settings = self_settings(vec![("web", on_self(echo, Protocol::Tcp))]);
    settings.self_allow = None;
    let srv = start_server(settings).await;
    assert_eq!(roundtrip(srv.services["web"], b"hello").await, b"hello");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_self_target_on_burrows_own_port_is_refused_at_runtime() {
    logs();
    let port = free_range(1);
    let mut settings = self_settings(vec![(
        "loop",
        Service {
            target: format!("localhost:{port}"),
            ..on_self("127.0.0.1:1".parse().unwrap(), Protocol::Tcp)
        },
    )]);
    settings.metrics = Some(SocketAddr::from(([127, 0, 0, 1], port)));
    let srv = start_server(settings).await;
    assert!(
        scrape(srv.metrics.unwrap()).is_ok(),
        "the metrics endpoint answers"
    );
    let answer = try_roundtrip(
        srv.services["loop"],
        b"GET /metrics HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        answer.as_ref().is_none_or(|a| a.is_empty()),
        "the self dialer must not connect to burrow's own metrics endpoint"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_private_self_service_serves_its_visitors() {
    logs();
    let echo = echo_server().await;
    let visitor_secret = Secret::generate();
    let mut settings = self_settings(vec![(
        "db",
        Service {
            clients: vec!["self".into()],
            ..private_service(echo, &["laptop"])
        },
    )]);
    settings
        .visitors
        .insert("laptop".into(), visitor_auth(visitor_secret.clone(), &[]));
    let srv = start_server(settings).await;
    let visitor = start_visitor(visitor_settings(
        srv.addr,
        visitor_secret,
        vec![("db", VisitTarget::service("db"))],
    ));
    let data = pattern(200_000, 2);
    assert_eq!(roundtrip(visitor.addr("db"), &data).await, data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn udp_on_self_reaches_its_target() {
    logs();
    let echo = udp_echo_server().await;
    let srv = start_server(self_settings(vec![("dns", on_self(echo, Protocol::Udp))])).await;
    let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let mut buf = [0u8; 64];
    let got = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            sock.send_to(b"query", srv.services["dns"]).await.unwrap();
            if let Ok(Ok((n, _))) =
                tokio::time::timeout(Duration::from_millis(300), sock.recv_from(&mut buf)).await
            {
                return n;
            }
        }
    })
    .await
    .expect("the UDP echo answers through self");
    assert_eq!(&buf[..got], b"query");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_port_of_a_self_range_leads_to_its_own_target() {
    logs();
    let targets = tagged_targets(3).await;
    let binds = free_range(3);
    let svc = Service {
        bind: Some(SocketAddr::from(([127, 0, 0, 1], binds))),
        target: format!("127.0.0.1:{targets}"),
        ports: 3,
        ..on_self("127.0.0.1:1".parse().unwrap(), Protocol::Tcp)
    };
    let _srv = start_server(self_settings(vec![("range", svc)])).await;
    for i in 0..3 {
        let at = SocketAddr::from(([127, 0, 0, 1], binds + i));
        assert_eq!(lands_on(at).await, targets + i);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failover_moves_between_self_and_a_remote_client() {
    logs();
    let echo = echo_server().await;
    let secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    settings.self_allow = Some(rules(&["10.0.0.0/8"]));
    settings.services.insert(
        "web".into(),
        Service {
            clients: vec!["self".into(), "home".into()],
            health: Some(Health {
                interval: Duration::from_millis(200),
                timeout: Duration::from_secs(1),
                failures: 1,
            }),
            ..service(echo, Protocol::Tcp)
        },
    );
    let srv = start_server(settings.clone()).await;
    let mut client = start_client(client_settings(srv.addr, secret.clone())).await;
    wait_state(&mut client.state, State::Connected).await;
    let web = srv.services["web"];
    let shared = srv.shared.clone();
    let down = |shared: &burrow::server::Shared, who: &str| {
        shared.status().services[0]
            .health
            .iter()
            .any(|h| h.client == who && !h.up)
    };
    wait_for(|| down(&shared, "self")).await;
    let held = open_echo(web).await;
    assert_eq!(client_open(&srv, "home"), 1, "self is down, so home serves");
    drop(held);
    wait_for(|| client_open(&srv, "home") == 0).await;

    settings.self_allow = Some(rules(&["127.0.0.0/8"]));
    srv.reload(settings.clone()).await;
    wait_for(|| !down(&shared, "self")).await;
    let held = open_echo(web).await;
    assert_eq!(client_open(&srv, "home"), 0, "self is up again and first");
    drop(held);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn self_takes_over_when_the_remote_client_cannot_reach_the_target() {
    logs();
    let echo = echo_server().await;
    let secret = Secret::generate();
    let mut settings = self_settings(vec![(
        "web",
        Service {
            clients: vec!["home".into(), "self".into()],
            health: Some(Health {
                interval: Duration::from_millis(200),
                timeout: Duration::from_secs(1),
                failures: 1,
            }),
            ..service(echo, Protocol::Tcp)
        },
    )]);
    settings.clients.insert(
        "home".into(),
        burrow::config::settings::ClientAuth {
            secrets: vec![secret.clone()],
            max_connections: None,
        },
    );
    let srv = start_server(settings).await;
    let mut c = client_settings(srv.addr, secret);
    c.allow = rules(&["10.0.0.0/8"]);
    let mut client = start_client(c).await;
    wait_state(&mut client.state, State::Connected).await;
    let shared = srv.shared.clone();
    wait_for(|| {
        shared.status().services[0]
            .health
            .iter()
            .any(|h| h.client == "home" && !h.up)
    })
    .await;
    let data = pattern(50_000, 4);
    assert_eq!(roundtrip(srv.services["web"], &data).await, data);
    assert_eq!(client_open(&srv, "home"), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bandwidth_and_max_connections_hold_on_a_self_service() {
    logs();
    let echo = echo_server().await;
    let svc = Service {
        bandwidth: Some(400_000),
        max_connections: Some(2),
        ..on_self(echo, Protocol::Tcp)
    };
    let srv = start_server(self_settings(vec![("web", svc)])).await;
    let web = srv.services["web"];
    let data = pattern(1_000_000, 5);
    let started = Instant::now();
    assert_eq!(roundtrip(web, &data).await, data);
    assert!(
        started.elapsed() >= Duration::from_millis(1800),
        "1 MB at 400 kB/s took {:?}",
        started.elapsed()
    );
    let first = open_echo(web).await;
    let second = open_echo(web).await;
    assert_ne!(
        try_roundtrip(web, b"third").await.as_deref(),
        Some(&b"third"[..]),
        "a third connection is over max_connections"
    );
    drop((first, second));
}

async fn proxy_aware_echo() -> (SocketAddr, Arc<Mutex<Vec<u8>>>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let keep = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = l.accept().await else { return };
            let keep = keep.clone();
            tokio::spawn(async move {
                let mut r = BufReader::new(s);
                let head = r.fill_buf().await.unwrap().to_vec();
                *keep.lock().unwrap() = head;
            });
        }
    });
    (addr, seen)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_self_target_gets_the_proxy_protocol_header() {
    logs();
    let (target, seen) = proxy_aware_echo().await;
    let svc = Service {
        proxy_protocol: Some(burrow::proxy_protocol::Version::V1),
        ..on_self(target, Protocol::Tcp)
    };
    let srv = start_server(self_settings(vec![("web", svc)])).await;
    let web = srv.services["web"];
    let mut user = TcpStream::connect(web).await.unwrap();
    let from = user.local_addr().unwrap();
    user.write_all(b"after").await.unwrap();
    wait_for(|| !seen.lock().unwrap().is_empty()).await;
    let head = String::from_utf8_lossy(&seen.lock().unwrap()).to_string();
    assert!(
        head.starts_with(&format!(
            "PROXY TCP4 {} {} {} {}\r\n",
            from.ip(),
            web.ip(),
            from.port(),
            web.port()
        )),
        "{head:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reload_adds_and_removes_self_services() {
    logs();
    let echo = echo_server().await;
    let mut settings = self_settings(vec![("old", on_self(echo, Protocol::Tcp))]);
    let srv = start_server(settings.clone()).await;
    let held = open_echo(srv.services["old"]).await;
    let port = free_range(1);
    let new_at = SocketAddr::from(([127, 0, 0, 1], port));
    settings.services.remove("old");
    settings.services.insert(
        "new".into(),
        Service {
            bind: Some(new_at),
            ..on_self(echo, Protocol::Tcp)
        },
    );
    srv.reload(settings).await;
    assert_cut(held).await;
    let mut answered = None;
    for _ in 0..100 {
        answered = try_roundtrip(new_at, b"added").await;
        if answered.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(answered.as_deref(), Some(&b"added"[..]));
}

async fn recording_relay(to: SocketAddr) -> (SocketAddr, Arc<Mutex<Vec<u8>>>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let wire = Arc::new(Mutex::new(Vec::new()));
    let all = wire.clone();
    tokio::spawn(async move {
        loop {
            let Ok((inside, _)) = l.accept().await else {
                return;
            };
            let Ok(outside) = TcpStream::connect(to).await else {
                return;
            };
            for (mut from, mut into) in [(inside, outside)]
                .map(|(a, b)| (a.into_split(), b.into_split()))
                .into_iter()
                .map(|((ar, aw), (br, bw))| ((ar, bw), (br, aw)))
                .flat_map(|(x, y)| [x, y])
            {
                let all = all.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 65536];
                    loop {
                        let n = match from.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => n,
                        };
                        all.lock().unwrap().extend_from_slice(&buf[..n]);
                        if into.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                    let _ = into.shutdown().await;
                });
            }
        }
    });
    (addr, wire)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_client_and_visitor_channels_stay_encrypted_next_to_self() {
    logs();
    let echo = echo_server().await;
    let secret = Secret::generate();
    let visitor_secret = Secret::generate();
    let mut settings = self_settings(vec![("local", on_self(echo, Protocol::Tcp))]);
    settings.clients.insert(
        "home".into(),
        burrow::config::settings::ClientAuth {
            secrets: vec![secret.clone()],
            max_connections: None,
        },
    );
    settings
        .services
        .insert("remote".into(), service(echo, Protocol::Tcp));
    settings.services.insert(
        "private".into(),
        Service {
            clients: vec!["self".into()],
            ..private_service(echo, &["laptop"])
        },
    );
    settings
        .visitors
        .insert("laptop".into(), visitor_auth(visitor_secret.clone(), &[]));
    let srv = start_server(settings).await;
    let (relay, wire) = recording_relay(srv.addr).await;
    let mut client = start_client(client_settings(relay, secret)).await;
    wait_state(&mut client.state, State::Connected).await;
    let shared = srv.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 1).await;
    let visitor = start_visitor(visitor_settings(
        relay,
        visitor_secret,
        vec![("private", VisitTarget::service("private"))],
    ));
    let marker = pattern(64 * 1024, 77);
    assert_eq!(roundtrip(srv.services["remote"], &marker).await, marker);
    assert_eq!(roundtrip(srv.services["local"], &marker).await, marker);
    assert_eq!(roundtrip(visitor.addr("private"), &marker).await, marker);
    let wire = wire.lock().unwrap().clone();
    assert!(wire.len() > 3 * marker.len(), "the relay saw the traffic");
    assert!(
        !wire
            .windows(64)
            .any(|w| marker.windows(64).take(64).any(|m| m == w)),
        "plaintext crossed the network"
    );
}
