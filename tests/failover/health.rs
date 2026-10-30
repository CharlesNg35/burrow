use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use burrow::config::settings::{ClientAuth, Health, Service};
use burrow::wire::protocol::VisitTarget;
use burrow::wire::secret::Secret;
use burrow_config::Protocol;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::common::*;
use super::selection::aside;
use super::{Farm, TARGET};

async fn recording_target() -> (SocketAddr, Arc<Mutex<Vec<Vec<u8>>>>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                return;
            };
            let log = log.clone();
            tokio::spawn(async move {
                let mut got = Vec::new();
                let _ = tokio::time::timeout(Duration::from_secs(3), s.read_to_end(&mut got)).await;
                log.lock().unwrap().push(got);
            });
        }
    });
    (addr, seen)
}

fn quick() -> Option<Health> {
    Some(Health {
        interval: Duration::from_millis(200),
        timeout: Duration::from_secs(1),
        failures: 1,
        http: None,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn health_probes_send_no_bytes_to_the_target_in_plaintext_mode() {
    let (target, seen) = recording_target().await;
    let secret = Secret::generate();
    let mut s = server_settings(vec![secret.clone()]);
    s.insecure_plaintext = true;
    let checked = Service {
        health: quick(),
        ..service(target, Protocol::Tcp)
    };
    let _t = tunnel_with(
        s,
        vec![("web", checked)],
        |mut c| {
            c.insecure_plaintext = true;
            c
        },
        secret,
    )
    .await;
    let probes = seen.clone();
    wait_for(|| probes.lock().unwrap().len() >= 2).await;
    let got = seen.lock().unwrap().clone();
    assert!(
        got.iter().all(|b| b.is_empty()),
        "health probes wrote bytes into the target: {got:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn adding_a_backup_client_keeps_open_visits() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let visitor_secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    settings.clients.insert(
        "spare".into(),
        ClientAuth {
            secrets: vec![Secret::generate()],
            max_connections: None,
        },
    );
    settings
        .visitors
        .insert("laptop".into(), visitor_auth(visitor_secret.clone(), &[]));
    let t = tunnel_with(
        settings.clone(),
        vec![("web", private_service(echo, &["laptop"]))],
        |c| c,
        secret,
    )
    .await;
    let v = start_visitor(visitor_settings(
        t.server.addr,
        visitor_secret,
        vec![("web", VisitTarget::service("web"))],
    ));
    let mut conn = open_echo(v.addr("web")).await;
    let mut next = settings.clone();
    let mut web = private_service(echo, &["laptop"]);
    web.clients.push("spare".into());
    next.services.insert("web".into(), web);
    t.server.reload_applied(next).await;
    conn.write_all(b"ping").await.unwrap();
    let mut buf = [0u8; 4];
    let r = tokio::time::timeout(Duration::from_secs(3), conn.read_exact(&mut buf)).await;
    assert!(
        matches!(r, Ok(Ok(_))) && &buf == b"ping",
        "the visit through client \"home\" was closed although home still serves web"
    );
}

struct Fixed(SocketAddr);

impl burrow::wire::transport::resolve::Resolver for Fixed {
    fn resolve<'a>(&'a self, host_port: &'a str) -> burrow::wire::transport::resolve::Resolved<'a> {
        Box::pin(async move {
            if host_port.starts_with("app.test:") {
                Ok(vec![self.0])
            } else {
                burrow::wire::transport::resolve::resolve(host_port).await
            }
        })
    }
}

pub(super) async fn tagged_udp_echo(tag: u8) -> SocketAddr {
    let s = udp_bind().await;
    let addr = s.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 2048];
        loop {
            let Ok((n, from)) = s.recv_from(&mut buf).await else {
                return;
            };
            let mut out = vec![tag];
            out.extend_from_slice(&buf[..n]);
            let _ = s.send_to(&out, from).await;
        }
    });
    addr
}

async fn named_client(
    server: SocketAddr,
    name: &str,
    secret: Secret,
    target: SocketAddr,
) -> RunningClient {
    let mut c = client_settings(server, secret);
    c.name = name.into();
    let client =
        burrow::client::Client::new(over_transport(c)).with_resolver(Arc::new(Fixed(target)));
    let mut running = start_client_with(client).await;
    wait_state(&mut running.state, burrow::client::State::Connected).await;
    running
}

pub(super) async fn udp_tag(sock: &tokio::net::UdpSocket) -> Option<u8> {
    let mut buf = [0u8; 64];
    for _ in 0..10 {
        sock.send(b"x").await.unwrap();
        if let Ok(Ok(n)) =
            tokio::time::timeout(Duration::from_millis(200), sock.recv(&mut buf)).await
            && n == 2
        {
            return Some(buf[0]);
        }
    }
    None
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_udp_service_moves_away_from_a_client_that_does_not_answer() {
    let dns = Service {
        clients: vec!["a".into(), "b".into()],
        target: TARGET.into(),
        ..service(tagged_udp_echo(b'A').await, Protocol::Udp)
    };
    let mut farm = Farm::start(dns, |_| {}).await;
    for (i, tag) in [(0, b'A'), (1, b'B')] {
        farm.disconnect(i).await;
        farm.sites[i].target = tagged_udp_echo(tag).await;
        farm.connect(i).await;
    }
    let s = udp_bind().await;
    s.connect(farm.server.services["web"]).await.unwrap();
    assert_eq!(udp_tag(&s).await, Some(b'A'));
    farm.connect_slow(0).await;
    let started = std::time::Instant::now();
    let mut tag = None;
    while started.elapsed() < Duration::from_millis(4500) && tag != Some(b'B') {
        tag = udp_tag(&s).await;
    }
    assert_eq!(tag, Some(b'B'), "the relay waited on the silent client");
    let shared = farm.server.shared.clone();
    wait_for(|| shared.status().services[0].set_aside == vec![aside("a", false)]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn udp_failover_returns_to_the_first_client() {
    logs();
    let (ea, eb) = (tagged_udp_echo(b'A').await, tagged_udp_echo(b'B').await);
    let (sa, sb) = (Secret::generate(), Secret::generate());
    let mut settings = server_settings(vec![]);
    settings.clients.clear();
    for (n, s) in [("a", &sa), ("b", &sb)] {
        settings.clients.insert(
            n.into(),
            ClientAuth {
                secrets: vec![s.clone()],
                max_connections: None,
            },
        );
    }
    let dns = Service {
        clients: vec!["a".into(), "b".into()],
        target: "app.test:53".into(),
        ..service(ea, Protocol::Udp)
    };
    settings.services.insert("dns".into(), dns);
    let server = start_server(settings).await;
    let b = named_client(server.addr, "b", sb.clone(), eb).await;
    let sock = udp_bind().await;
    sock.connect(server.services["dns"]).await.unwrap();
    let mut first = None;
    for _ in 0..20 {
        first = udp_tag(&sock).await;
        if first.is_some() {
            break;
        }
    }
    assert_eq!(first, Some(b'B'));
    let a = named_client(server.addr, "a", sa.clone(), ea).await;
    let shared = server.shared.clone();
    wait_for(|| shared.idle_channels("a") >= 1).await;
    let mut now = None;
    for _ in 0..30 {
        now = udp_tag(&sock).await;
        if now == Some(b'A') {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        now,
        Some(b'A'),
        "the UDP relay stays on the backup client after the first one is back"
    );
    drop((a, b));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn many_probes_leave_no_open_connections_or_drained_pool() {
    let (target, seen) = recording_target().await;
    let secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    settings.metrics = Some("127.0.0.1:0".parse().unwrap());
    let checked = Service {
        health: Some(Health {
            interval: Duration::from_millis(1),
            timeout: Duration::from_secs(1),
            failures: 1,
            http: None,
        }),
        ..service(target, Protocol::Tcp)
    };
    let t = tunnel_with(settings, vec![("web", checked)], |c| c, secret).await;
    let probes = seen.clone();
    wait_for(|| probes.lock().unwrap().len() >= 10).await;
    let shared = t.server.shared.clone();
    assert_eq!(shared.active_connections(), 0);
    assert_eq!(shared.waiting_visitors(), 0);
    assert!(shared.idle_channels("home") <= 64);
    let s = t.server.shared.status();
    assert_eq!(s.clients[0].open, 0);
    assert_eq!(
        (
            s.services[0].total,
            s.services[0].open,
            s.services[0].bytes_in
        ),
        (0, 0, 0)
    );
    assert!(s.services[0].health[0].up);
    assert!(seen.lock().unwrap().iter().all(|b| b.is_empty()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_denied_target_marks_the_only_client_down_but_still_tries_it() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    settings.metrics = Some("127.0.0.1:0".parse().unwrap());
    let checked = Service {
        health: quick(),
        ..service(echo, Protocol::Tcp)
    };
    let t = tunnel_with(
        settings,
        vec![("web", checked)],
        |mut c| {
            c.allow =
                burrow::policy::allowlist::AllowList::Rules(vec!["10.0.0.0/8".parse().unwrap()]);
            c
        },
        secret,
    )
    .await;
    let mut down = false;
    for _ in 0..50 {
        let s = t.server.shared.status();
        if s.services[0].health.first().is_some_and(|h| !h.up) {
            down = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(down);
    let _ = try_roundtrip(t.server.services["web"], b"x").await;
    let s = t.server.shared.status();
    assert_eq!((s.services[0].total, s.services[0].refused), (1, 0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn balance_stays_even_while_one_client_is_down() {
    let ea = tcp_tag_echo(b'A').await;
    let eb = tcp_tag_echo(b'B').await;
    let (_held, dead) = refusing();
    let names = [
        ("a", Secret::generate(), ea),
        ("b", Secret::generate(), eb),
        ("c", Secret::generate(), dead),
    ];
    let mut settings = server_settings(vec![]);
    settings.clients.clear();
    for (n, s, _) in &names {
        settings.clients.insert(
            n.to_string(),
            ClientAuth {
                secrets: vec![s.clone()],
                max_connections: None,
            },
        );
    }
    let web = Service {
        clients: vec!["a".into(), "b".into(), "c".into()],
        mode: burrow_config::Mode::Balance,
        target: "app.test:80".into(),
        health: quick(),
        ..service(ea, Protocol::Tcp)
    };
    settings.services.insert("web".into(), web);
    let server = start_server(settings).await;
    let mut running = Vec::new();
    for (n, s, target) in &names {
        running.push(named_client(server.addr, n, s.clone(), *target).await);
    }
    let shared = server.shared.clone();
    wait_for(|| {
        shared.status().services[0]
            .health
            .iter()
            .any(|h| h.client == "c" && !h.up)
    })
    .await;
    let mut seen = std::collections::BTreeMap::new();
    for _ in 0..20 {
        *seen
            .entry(first_byte(server.services["web"]).await)
            .or_insert(0) += 1;
    }
    assert_eq!(seen.get(&Some(b'A')), Some(&10), "{seen:?}");
    assert_eq!(seen.get(&Some(b'B')), Some(&10), "{seen:?}");
    drop(running);
}

async fn tcp_tag_echo(tag: u8) -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let _ = s.write_all(&[tag]).await;
                let mut sink = Vec::new();
                let _ = s.read_to_end(&mut sink).await;
            });
        }
    });
    addr
}

async fn first_byte(addr: SocketAddr) -> Option<u8> {
    let mut s = TcpStream::connect(addr).await.ok()?;
    let mut b = [0u8; 1];
    tokio::time::timeout(Duration::from_secs(3), s.read_exact(&mut b))
        .await
        .ok()?
        .ok()?;
    Some(b[0])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reconnecting_client_with_a_dead_target_does_not_take_traffic_back() {
    logs();
    let (_held, dead) = refusing();
    let eb = tcp_tag_echo(b'B').await;
    let (sa, sb) = (Secret::generate(), Secret::generate());
    let mut settings = server_settings(vec![]);
    settings.clients.clear();
    for (n, s) in [("a", &sa), ("b", &sb)] {
        settings.clients.insert(
            n.into(),
            ClientAuth {
                secrets: vec![s.clone()],
                max_connections: None,
            },
        );
    }
    let web = Service {
        clients: vec!["a".into(), "b".into()],
        target: "app.test:80".into(),
        health: Some(Health {
            interval: Duration::from_millis(500),
            timeout: Duration::from_secs(1),
            failures: 2,
            http: None,
        }),
        ..service(eb, Protocol::Tcp)
    };
    settings.services.insert("web".into(), web);
    let server = start_server(settings).await;
    let b = named_client(server.addr, "b", sb.clone(), eb).await;
    let a = named_client(server.addr, "a", sa.clone(), dead).await;
    let shared = server.shared.clone();
    wait_for(|| {
        shared.status().services[0]
            .health
            .iter()
            .any(|h| h.client == "a" && !h.up)
    })
    .await;
    assert_eq!(first_byte(server.services["web"]).await, Some(b'B'));
    a.stop.send(true).unwrap();
    a.task.await.unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    let a = named_client(server.addr, "a", sa.clone(), dead).await;
    wait_for(|| shared.idle_channels("a") >= 1).await;
    assert_eq!(
        first_byte(server.services["web"]).await,
        Some(b'B'),
        "after a reconnect, client a counts as up again until it fails the check twice"
    );
    drop((a, b));
}
