mod common;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use burrow::allowlist::AllowList;
use burrow::client::{Client, State};
use burrow::config::model::{Mode, Protocol};
use burrow::config::settings::{ClientAuth, Health, ServerSettings, Service};
use burrow::metrics::SetAside;
use burrow::protocol::VisitTarget;
use burrow::secret::Secret;
use burrow::transport::resolve::{Resolved, Resolver};
use common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::task::JoinHandle;

const TARGET: &str = "app.test:80";

struct Local(SocketAddr);

impl Resolver for Local {
    fn resolve<'a>(&'a self, host_port: &'a str) -> Resolved<'a> {
        Box::pin(async move {
            if host_port == TARGET {
                Ok(vec![self.0])
            } else {
                burrow::transport::resolve::resolve(host_port).await
            }
        })
    }
}

struct Slow(SocketAddr);

impl Resolver for Slow {
    fn resolve<'a>(&'a self, host_port: &'a str) -> Resolved<'a> {
        Box::pin(async move {
            if host_port != TARGET {
                return burrow::transport::resolve::resolve(host_port).await;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok(vec![self.0])
        })
    }
}

async fn tagged_echo_on(addr: SocketAddr, tag: u8) -> (SocketAddr, JoinHandle<()>) {
    let l = TcpListener::bind(addr).await.unwrap();
    let addr = l.local_addr().unwrap();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                return;
            };
            tokio::spawn(async move {
                if s.write_all(&[tag]).await.is_err() {
                    return;
                }
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    (addr, task)
}

struct Site {
    name: &'static str,
    secret: Secret,
    target: SocketAddr,
    echo: JoinHandle<()>,
    client: Option<RunningClient>,
}

struct Farm {
    server: RunningServer,
    sites: Vec<Site>,
    plaintext: bool,
}

impl Farm {
    async fn start(service: Service, extra: impl FnOnce(&mut ServerSettings)) -> Farm {
        logs();
        let mut sites = Vec::new();
        for (name, tag) in [("a", b'A'), ("b", b'B')] {
            let (target, echo) = tagged_echo_on("127.0.0.1:0".parse().unwrap(), tag).await;
            sites.push(Site {
                name,
                secret: Secret::generate(),
                target,
                echo,
                client: None,
            });
        }
        let mut settings = server_settings(vec![]);
        settings.clients = sites
            .iter()
            .map(|s| {
                (
                    s.name.to_string(),
                    ClientAuth {
                        secrets: vec![s.secret.clone()],
                        max_connections: None,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        settings.services.insert("web".into(), service);
        extra(&mut settings);
        let plaintext = settings.insecure_plaintext;
        let server = start_server(settings).await;
        let mut farm = Farm {
            server,
            sites,
            plaintext,
        };
        for i in 0..farm.sites.len() {
            farm.connect(i).await;
        }
        farm
    }

    async fn connect(&mut self, i: usize) {
        self.connect_allowing(i, "127.0.0.0/8").await;
    }

    async fn connect_refusing(&mut self, i: usize) {
        self.disconnect(i).await;
        self.connect_allowing(i, "10.0.0.0/8").await;
    }

    async fn connect_allowing(&mut self, i: usize, allow: &str) {
        let target = self.sites[i].target;
        self.connect_using(i, allow, Arc::new(Local(target))).await;
    }

    async fn connect_slow(&mut self, i: usize) {
        self.disconnect(i).await;
        let target = self.sites[i].target;
        self.connect_using(i, "127.0.0.0/8", Arc::new(Slow(target)))
            .await;
    }

    async fn connect_using(&mut self, i: usize, allow: &str, resolver: Arc<dyn Resolver>) {
        let site = &self.sites[i];
        let mut c = client_settings(self.server.addr, site.secret.clone());
        c.name = site.name.into();
        c.allow = AllowList::Rules(vec![allow.parse().unwrap()]);
        c.insecure_plaintext = self.plaintext;
        let client = Client::new(over_transport(c)).with_resolver(resolver);
        let mut running = start_client_with(client).await;
        wait_state(&mut running.state, State::Connected).await;
        let shared = self.server.shared.clone();
        let name = site.name;
        wait_for(|| shared.idle_channels(name) >= 1).await;
        self.sites[i].client = Some(running);
    }

    async fn disconnect(&mut self, i: usize) {
        if let Some(c) = self.sites[i].client.take() {
            c.stop.send(true).unwrap();
            c.task.await.unwrap();
        }
        let shared = self.server.shared.clone();
        let name = self.sites[i].name.to_string();
        wait_for(|| !shared.session_names().contains(&name)).await;
    }

    async fn stop_target(&mut self, i: usize) {
        self.sites[i].echo.abort();
        let _ = (&mut self.sites[i].echo).await;
    }

    async fn restart_target(&mut self, i: usize) {
        let tag = if i == 0 { b'A' } else { b'B' };
        let (_, echo) = tagged_echo_on(self.sites[i].target, tag).await;
        self.sites[i].echo = echo;
    }
}

async fn served_by(addr: SocketAddr) -> Option<char> {
    let mut s = TcpStream::connect(addr).await.ok()?;
    let mut tag = [0u8; 1];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut tag))
        .await
        .ok()?
        .ok()?;
    s.write_all(b"ping").await.ok()?;
    let mut back = [0u8; 4];
    s.read_exact(&mut back).await.ok()?;
    (&back == b"ping").then_some(tag[0] as char)
}

async fn eventually_served_by(addr: SocketAddr, want: char) {
    for _ in 0..100 {
        if served_by(addr).await == Some(want) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("connections never reached client {want}");
}

fn web(mode: Mode, health: Option<Health>) -> Service {
    Service {
        clients: vec!["a".into(), "b".into()],
        mode,
        health,
        target: TARGET.into(),
        ..service("127.0.0.1:1".parse().unwrap(), Protocol::Tcp)
    }
}

fn quick_health() -> Option<Health> {
    Some(Health {
        interval: Duration::from_millis(300),
        timeout: Duration::from_secs(1),
        failures: 1,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failover_uses_the_first_client_and_moves_while_it_is_away() {
    let mut farm = Farm::start(web(Mode::Failover, None), |_| {}).await;
    let addr = farm.server.services["web"];
    for _ in 0..5 {
        assert_eq!(served_by(addr).await, Some('A'));
    }
    let open = TcpStream::connect(addr).await.unwrap();
    farm.disconnect(0).await;
    for _ in 0..5 {
        assert_eq!(served_by(addr).await, Some('B'));
    }
    farm.connect(0).await;
    eventually_served_by(addr, 'A').await;
    drop(open);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn balance_spreads_new_connections_over_the_clients() {
    let farm = Farm::start(web(Mode::Balance, None), |_| {}).await;
    let addr = farm.server.services["web"];
    let mut seen = BTreeMap::new();
    for _ in 0..20 {
        *seen.entry(served_by(addr).await.unwrap()).or_insert(0) += 1;
    }
    assert_eq!(seen.get(&'A'), Some(&10), "{seen:?}");
    assert_eq!(seen.get(&'B'), Some(&10), "{seen:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn health_checks_skip_a_client_whose_target_is_down() {
    let mut farm = Farm::start(web(Mode::Failover, quick_health()), |_| {}).await;
    let addr = farm.server.services["web"];
    eventually_served_by(addr, 'A').await;
    farm.stop_target(0).await;
    eventually_served_by(addr, 'B').await;
    for _ in 0..5 {
        assert_eq!(served_by(addr).await, Some('B'));
    }
    farm.restart_target(0).await;
    eventually_served_by(addr, 'A').await;
}

fn set_aside(farm: &Farm) -> Vec<SetAside> {
    farm.server.shared.status().services[0].set_aside.clone()
}

fn aside(client: &str, refused: bool) -> SetAside {
    SetAside {
        client: client.into(),
        refused,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_down_target_is_skipped_for_each_connection() {
    let mut farm = Farm::start(web(Mode::Failover, None), |_| {}).await;
    let addr = farm.server.services["web"];
    farm.stop_target(0).await;
    for _ in 0..5 {
        assert_eq!(served_by(addr).await, Some('B'));
    }
    assert_eq!(set_aside(&farm), vec![aside("a", false)]);
    farm.stop_target(1).await;
    assert_eq!(served_by(addr).await, None);
    farm.restart_target(0).await;
    assert_eq!(
        served_by(addr).await,
        Some('A'),
        "a client set aside is still tried when the others fail"
    );
    assert_eq!(set_aside(&farm), vec![aside("b", false)]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_that_refuses_the_target_is_skipped_until_it_reconnects() {
    for mode in [Mode::Failover, Mode::Balance] {
        let mut farm = Farm::start(web(mode, None), |_| {}).await;
        let addr = farm.server.services["web"];
        farm.connect_refusing(0).await;
        for _ in 0..10 {
            assert_eq!(served_by(addr).await, Some('B'), "{mode:?}");
        }
        assert_eq!(set_aside(&farm), vec![aside("a", true)]);
        farm.disconnect(1).await;
        assert_eq!(
            served_by(addr).await,
            None,
            "with only a refusing client left, connections fail as before"
        );
        farm.connect(1).await;
        farm.disconnect(0).await;
        farm.connect(0).await;
        assert!(set_aside(&farm).is_empty());
        let mut seen = BTreeMap::new();
        for _ in 0..10 {
            *seen.entry(served_by(addr).await.unwrap()).or_insert(0) += 1;
        }
        let want = match mode {
            Mode::Failover => BTreeMap::from([('A', 10)]),
            Mode::Balance => BTreeMap::from([('A', 5), ('B', 5)]),
        };
        assert_eq!(seen, want, "{mode:?}");
    }
}

async fn echoed_through(addr: SocketAddr, data: Vec<u8>) -> Vec<u8> {
    let s = TcpStream::connect(addr).await.unwrap();
    let (mut r, mut w) = s.into_split();
    let writer = tokio::spawn(async move {
        w.write_all(&data).await.unwrap();
        w.shutdown().await.unwrap();
    });
    let mut back = Vec::new();
    tokio::time::timeout(Duration::from_secs(20), r.read_to_end(&mut back))
        .await
        .unwrap()
        .unwrap();
    writer.await.unwrap();
    back
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn what_the_visitor_sent_first_reaches_the_next_client_intact() {
    for plaintext in [false, true] {
        let mut farm = Farm::start(web(Mode::Failover, None), |s| {
            s.insecure_plaintext = plaintext;
        })
        .await;
        let addr = farm.server.services["web"];
        for refusing in [true, false] {
            if refusing {
                farm.connect_refusing(0).await;
            } else {
                farm.disconnect(0).await;
                farm.connect(0).await;
                farm.stop_target(0).await;
            }
            for size in [0usize, 10, 70_000, 1_000_000] {
                let data: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
                let back = echoed_through(addr, data.clone()).await;
                assert_eq!(
                    back.first(),
                    Some(&b'B'),
                    "plaintext {plaintext}, {size} bytes"
                );
                assert!(
                    back[1..] == data[..],
                    "plaintext {plaintext}, refusing {refusing}: {size} bytes sent, {} came back",
                    back.len() - 1
                );
            }
        }
        let shared = farm.server.shared.clone();
        wait_for(|| shared.active_connections() == 0).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bytes_a_client_got_before_it_went_silent_are_never_sent_to_another() {
    let mut farm = Farm::start(web(Mode::Failover, None), |_| {}).await;
    let addr = farm.server.services["web"];
    farm.connect_slow(0).await;
    let back = echoed_through(addr, b"POST /pay".to_vec()).await;
    assert!(
        back.is_empty(),
        "the bytes may already be on their way to the target through client a: {back:?}"
    );
    assert_eq!(set_aside(&farm), vec![aside("a", false)]);
    assert_eq!(served_by(addr).await, Some('B'));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_visitor_that_sent_nothing_yet_moves_on_from_a_silent_client() {
    let mut farm = Farm::start(web(Mode::Failover, None), |_| {}).await;
    let addr = farm.server.services["web"];
    farm.connect_slow(0).await;
    assert_eq!(served_by(addr).await, Some('B'));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refusing_client_is_skipped_for_visitors_too() {
    let visitor_secret = Secret::generate();
    let private = Service {
        bind: None,
        visitors: vec!["laptop".into()],
        ..web(Mode::Failover, None)
    };
    let mut farm = Farm::start(private, |s| {
        s.visitors
            .insert("laptop".into(), visitor_auth(visitor_secret.clone(), &[]));
    })
    .await;
    farm.connect_refusing(0).await;
    let v = start_visitor(visitor_settings(
        farm.server.addr,
        visitor_secret,
        vec![("web", VisitTarget::service("web"))],
    ));
    for _ in 0..5 {
        assert_eq!(served_by(v.addr("web")).await, Some('B'));
    }
    let data: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
    let back = echoed_through(v.addr("web"), data.clone()).await;
    assert_eq!(back.first(), Some(&b'B'));
    assert!(back[1..] == data[..], "{} bytes came back", back.len() - 1);
    assert_eq!(set_aside(&farm), vec![aside("a", true)]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_udp_service_moves_away_from_a_refusing_client() {
    let ta = tagged_udp_echo(b'A').await;
    let dns = Service {
        clients: vec!["a".into(), "b".into()],
        target: TARGET.into(),
        ..service(ta, Protocol::Udp)
    };
    let mut farm = Farm::start(dns, |_| {}).await;
    for (i, tag) in [(0, b'A'), (1, b'B')] {
        farm.disconnect(i).await;
        farm.sites[i].target = tagged_udp_echo(tag).await;
        farm.connect(i).await;
    }
    let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    s.connect(farm.server.services["web"]).await.unwrap();
    assert_eq!(udp_tag(&s).await, Some(b'A'));
    farm.connect_refusing(0).await;
    let mut tag = None;
    for _ in 0..20 {
        tag = udp_tag(&s).await;
        if tag == Some(b'B') {
            break;
        }
    }
    assert_eq!(tag, Some(b'B'));
    let shared = farm.server.shared.clone();
    wait_for(|| shared.status().services[0].set_aside == vec![aside("a", true)]).await;
    assert_eq!(
        udp_tag(&s).await,
        Some(b'B'),
        "the relay does not move back to the refusing client"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn visitors_fail_over_too() {
    let visitor_secret = Secret::generate();
    let private = Service {
        bind: None,
        visitors: vec!["laptop".into()],
        ..web(Mode::Failover, None)
    };
    let mut farm = Farm::start(private, |s| {
        s.visitors
            .insert("laptop".into(), visitor_auth(visitor_secret.clone(), &[]));
    })
    .await;
    let v = start_visitor(visitor_settings(
        farm.server.addr,
        visitor_secret,
        vec![("web", VisitTarget::service("web"))],
    ));
    assert_eq!(served_by(v.addr("web")).await, Some('A'));
    farm.disconnect(0).await;
    assert_eq!(served_by(v.addr("web")).await, Some('B'));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_udp_service_uses_whichever_client_is_connected() {
    let echo = udp_echo_server().await;
    let dns = Service {
        clients: vec!["a".into(), "b".into()],
        target: echo.to_string(),
        ..service(echo, Protocol::Udp)
    };
    let mut farm = Farm::start(dns, |_| {}).await;
    farm.disconnect(0).await;
    let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    s.connect(farm.server.services["web"]).await.unwrap();
    let mut buf = [0u8; 64];
    for _ in 0..50 {
        s.send(b"hello").await.unwrap();
        if let Ok(Ok(n)) = tokio::time::timeout(Duration::from_millis(200), s.recv(&mut buf)).await
            && &buf[..n] == b"hello"
        {
            return;
        }
    }
    panic!("the UDP service must work through the remaining client");
}

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
    t.server.reload(next).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    conn.write_all(b"ping").await.unwrap();
    let mut buf = [0u8; 4];
    let r = tokio::time::timeout(Duration::from_secs(3), conn.read_exact(&mut buf)).await;
    assert!(
        matches!(r, Ok(Ok(_))) && &buf == b"ping",
        "the visit through client \"home\" was closed although home still serves web"
    );
}

struct Fixed(SocketAddr);

impl burrow::transport::resolve::Resolver for Fixed {
    fn resolve<'a>(&'a self, host_port: &'a str) -> burrow::transport::resolve::Resolved<'a> {
        Box::pin(async move {
            if host_port.starts_with("app.test:") {
                Ok(vec![self.0])
            } else {
                burrow::transport::resolve::resolve(host_port).await
            }
        })
    }
}

async fn tagged_udp_echo(tag: u8) -> SocketAddr {
    let s = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
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

async fn udp_tag(sock: &tokio::net::UdpSocket) -> Option<u8> {
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
    let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
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
    let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
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
            c.allow = burrow::allowlist::AllowList::Rules(vec!["10.0.0.0/8".parse().unwrap()]);
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
        mode: burrow::config::model::Mode::Balance,
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
