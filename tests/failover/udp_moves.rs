use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use burrow::config::settings::Service;
use burrow::policy::allowlist::AllowList;
use burrow::wire::secret::Secret;
use burrow_config::{Mode, Protocol};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use super::common::*;
use super::health::{tagged_udp_echo, udp_tag};
use super::selection::aside;
use super::{Farm, TARGET, quick_health, web};

async fn deaf_client(
    server: SocketAddr,
    name: &str,
    secret: Secret,
    channels: usize,
) -> JoinHandle<()> {
    use burrow::wire::protocol::{ChannelKind, ClientMsg, Hello, Preamble, ServerMsg, Status};
    let pre = |kind| Preamble {
        kind,
        cipher: burrow_config::Cipher::Chachapoly,
        plaintext: false,
        name: name.to_string(),
    };
    let s = TcpStream::connect(server).await.unwrap();
    let mut control = burrow::wire::handshake::initiate(
        s.into(),
        &pre(ChannelKind::Control),
        &secret,
        &Hello::Control {
            version: 1,
            instance: [3; 16],
        },
    )
    .await
    .unwrap();
    let token = loop {
        let rec = control
            .reader
            .recv(&mut control.stream)
            .await
            .unwrap()
            .unwrap();
        if let Ok(ServerMsg::Welcome { token, .. }) = ServerMsg::decode(rec) {
            break token;
        }
    };
    let mut held = Vec::new();
    for _ in 0..channels {
        let s = TcpStream::connect(server).await.unwrap();
        let ch = burrow::wire::handshake::initiate(
            s.into(),
            &pre(ChannelKind::Data),
            &secret,
            &Hello::Data { version: 1, token },
        )
        .await
        .unwrap();
        held.push(tokio::spawn(async move {
            let mut ch = ch;
            if ch
                .reader
                .recv(&mut ch.stream)
                .await
                .ok()
                .flatten()
                .is_some()
            {
                let _ = ch.writer.send(&mut ch.stream, &[Status::Ok as u8]).await;
            }
            std::future::pending::<()>().await;
            drop(ch);
        }));
    }
    tokio::spawn(async move {
        let _held = held;
        while let Ok(Some(rec)) = control.reader.recv(&mut control.stream).await {
            if matches!(ServerMsg::decode(rec), Ok(ServerMsg::Ping)) {
                let pong = ClientMsg::Pong.encode();
                if control
                    .writer
                    .send(&mut control.stream, &pong)
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_udp_service_moves_away_from_a_client_that_stops_reading() {
    if ws_mode() {
        return;
    }
    let dns = Service {
        clients: vec!["a".into(), "b".into()],
        target: TARGET.into(),
        ..service(tagged_udp_echo(b'A').await, Protocol::Udp)
    };
    let mut farm = Farm::start(dns, |_| {}).await;
    farm.disconnect(0).await;
    farm.disconnect(1).await;
    farm.sites[1].target = tagged_udp_echo(b'B').await;
    let _deaf = deaf_client(farm.server.addr, "a", farm.sites[0].secret.clone(), 4).await;
    let shared = farm.server.shared.clone();
    wait_for(|| shared.idle_channels("a") >= 1).await;
    let web = farm.server.services["web"];
    let flood = tokio::spawn(async move {
        let s = udp_bind().await;
        s.connect(web).await.unwrap();
        let data = vec![7u8; 60_000];
        loop {
            let _ = s.send(&data).await;
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    });
    farm.connect(1).await;
    let shared = farm.server.shared.clone();
    wait_for(|| shared.status().services[0].set_aside == vec![aside("a", false)]).await;
    flood.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_udp_service_moves_away_from_a_client_that_gives_no_data_channel() {
    if ws_mode() {
        return;
    }
    let dns = Service {
        clients: vec!["a".into(), "b".into()],
        target: TARGET.into(),
        ..service(tagged_udp_echo(b'A').await, Protocol::Udp)
    };
    let mut farm = Farm::start(dns, |_| {}).await;
    farm.disconnect(0).await;
    farm.disconnect(1).await;
    farm.sites[1].target = tagged_udp_echo(b'B').await;
    let _deaf = deaf_client(farm.server.addr, "a", farm.sites[0].secret.clone(), 0).await;
    let shared = farm.server.shared.clone();
    wait_for(|| shared.session_names().contains(&"a".to_string())).await;
    farm.connect(1).await;
    let s = udp_bind().await;
    s.connect(farm.server.services["web"]).await.unwrap();
    let started = std::time::Instant::now();
    let mut tag = None;
    while started.elapsed() < Duration::from_secs(20) && tag != Some(b'B') {
        tag = udp_tag(&s).await;
    }
    assert_eq!(
        tag,
        Some(b'B'),
        "the relay kept waiting for a data channel of client a"
    );
}

#[cfg(target_os = "linux")]
fn sockets_to(port: u16) -> usize {
    let ours: std::collections::HashSet<String> = std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter_map(|e| std::fs::read_link(e.ok()?.path()).ok())
        .filter_map(|l| {
            let l = l.to_string_lossy().to_string();
            Some(l.strip_prefix("socket:[")?.strip_suffix(']')?.to_string())
        })
        .collect();
    ["/proc/self/net/tcp", "/proc/self/net/tcp6"]
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .flat_map(|t| t.lines().skip(1).map(str::to_string).collect::<Vec<_>>())
        .filter(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            let remote = f.get(2).and_then(|a| a.rsplit(':').next());
            remote.and_then(|p| u16::from_str_radix(p, 16).ok()) == Some(port)
                && f.get(9).is_some_and(|i| ours.contains(*i))
        })
        .count()
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_health_probe_given_up_in_plaintext_leaves_no_target_connection_behind() {
    let mut farm = Farm::start(web(Mode::Failover, quick_health()), |s| {
        s.insecure_plaintext = true
    })
    .await;
    farm.stop_target(0).await;
    let target = farm.sites[0].target;
    let silent = TcpListener::bind(target).await.unwrap();
    let held = tokio::spawn(async move {
        let mut kept = Vec::new();
        while let Ok((s, _)) = silent.accept().await {
            kept.push(s);
        }
    });
    farm.connect_slow(0).await;
    tokio::time::sleep(Duration::from_secs(9)).await;
    let left = sockets_to(target.port());
    held.abort();
    assert_eq!(
        left, 0,
        "the client still holds target connections for probes the server gave up"
    );
}

struct Moving(SocketAddr, SocketAddr, Arc<std::sync::atomic::AtomicBool>);

impl burrow::wire::transport::resolve::Resolver for Moving {
    fn resolve<'a>(&'a self, host_port: &'a str) -> burrow::wire::transport::resolve::Resolved<'a> {
        Box::pin(async move {
            if !host_port.starts_with("app.test:") {
                return burrow::wire::transport::resolve::resolve(host_port).await;
            }
            if self.2.load(std::sync::atomic::Ordering::Relaxed) {
                Ok(vec![self.1])
            } else {
                Ok(vec![self.0])
            }
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_steady_udp_sender_follows_its_target_to_a_new_address() {
    let (ea, eb) = (tagged_udp_echo(b'A').await, tagged_udp_echo(b'B').await);
    let secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    let app = Service {
        target: "app.test:53".into(),
        ..service(ea, Protocol::Udp)
    };
    settings.services.insert("app".into(), app);
    let server = start_server(settings).await;
    let moved = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let c = client_settings(server.addr, secret);
    let client = burrow::client::Client::new(over_transport(c)).with_resolver(Arc::new(Moving(
        ea,
        eb,
        moved.clone(),
    )));
    let mut running = start_client_with(client).await;
    wait_state(&mut running.state, burrow::client::State::Connected).await;
    let sock = udp_bind().await;
    sock.connect(server.services["app"]).await.unwrap();
    let mut first = None;
    for _ in 0..20 {
        first = udp_tag(&sock).await;
        if first.is_some() {
            break;
        }
    }
    assert_eq!(first, Some(b'A'));
    moved.store(true, std::sync::atomic::Ordering::Relaxed);
    let started = std::time::Instant::now();
    let mut now = first;
    while started.elapsed() < Duration::from_secs(45) && now != Some(b'B') {
        now = udp_tag(&sock).await.or(now);
    }
    assert_eq!(
        now,
        Some(b'B'),
        "a sender that never paused stayed on the address its target left"
    );
}

struct Rotating(SocketAddr, SocketAddr, std::sync::atomic::AtomicUsize);

impl burrow::wire::transport::resolve::Resolver for Rotating {
    fn resolve<'a>(&'a self, host_port: &'a str) -> burrow::wire::transport::resolve::Resolved<'a> {
        Box::pin(async move {
            if !host_port.starts_with("app.test:") {
                return burrow::wire::transport::resolve::resolve(host_port).await;
            }
            if self
                .2
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                .is_multiple_of(2)
            {
                Ok(vec![self.0, self.1])
            } else {
                Ok(vec![self.1, self.0])
            }
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_steady_udp_sender_stays_put_while_round_robin_dns_rotates() {
    let (ea, eb) = (tagged_udp_echo(b'A').await, tagged_udp_echo(b'B').await);
    let secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    let app = Service {
        target: "app.test:53".into(),
        ..service(ea, Protocol::Udp)
    };
    settings.services.insert("app".into(), app);
    let server = start_server(settings).await;
    let c = client_settings(server.addr, secret);
    let client = burrow::client::Client::new(over_transport(c)).with_resolver(Arc::new(Rotating(
        ea,
        eb,
        std::sync::atomic::AtomicUsize::new(0),
    )));
    let mut running = start_client_with(client).await;
    wait_state(&mut running.state, burrow::client::State::Connected).await;
    let sock = udp_bind().await;
    sock.connect(server.services["app"]).await.unwrap();
    let mut tags = std::collections::BTreeSet::new();
    let started = std::time::Instant::now();
    while started.elapsed() < Duration::from_secs(40) {
        if let Some(t) = udp_tag(&sock).await {
            tags.insert(t);
        }
    }
    assert_eq!(
        tags.len(),
        1,
        "a steady sender moved between addresses: {tags:?}"
    );
}

struct Phased(SocketAddr, SocketAddr, Arc<std::sync::atomic::AtomicUsize>);

impl burrow::wire::transport::resolve::Resolver for Phased {
    fn resolve<'a>(&'a self, host_port: &'a str) -> burrow::wire::transport::resolve::Resolved<'a> {
        Box::pin(async move {
            if !host_port.starts_with("app.test:") {
                return burrow::wire::transport::resolve::resolve(host_port).await;
            }
            match self.2.load(std::sync::atomic::Ordering::Relaxed) {
                1 => Ok(vec![self.1]),
                _ => Ok(vec![self.0, self.1]),
            }
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_udp_relay_returns_to_ipv4_after_one_ipv6_only_answer() {
    let ea = tagged_udp_echo(b'A').await;
    let six = tokio::net::UdpSocket::bind("[::1]:0").await.unwrap();
    let eb = six.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 2048];
        while let Ok((n, from)) = six.recv_from(&mut buf).await {
            let mut out = vec![b'B'];
            out.extend_from_slice(&buf[..n]);
            let _ = six.send_to(&out, from).await;
        }
    });
    let secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    let app = Service {
        target: "app.test:53".into(),
        ..service(ea, Protocol::Udp)
    };
    settings.services.insert("app".into(), app);
    let server = start_server(settings).await;
    let phase = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut c = client_settings(server.addr, secret);
    c.allow = AllowList::Rules(vec![
        "127.0.0.0/8".parse().unwrap(),
        "::1/128".parse().unwrap(),
    ]);
    let client = burrow::client::Client::new(over_transport(c)).with_resolver(Arc::new(Phased(
        ea,
        eb,
        phase.clone(),
    )));
    let mut running = start_client_with(client).await;
    wait_state(&mut running.state, burrow::client::State::Connected).await;
    let sock = udp_bind().await;
    sock.connect(server.services["app"]).await.unwrap();
    let mut now = None;
    for _ in 0..20 {
        now = udp_tag(&sock).await;
        if now.is_some() {
            break;
        }
    }
    assert_eq!(now, Some(b'A'));
    phase.store(1, std::sync::atomic::Ordering::Relaxed);
    let started = std::time::Instant::now();
    while started.elapsed() < Duration::from_secs(45) && now != Some(b'B') {
        now = udp_tag(&sock).await.or(now);
    }
    assert_eq!(
        now,
        Some(b'B'),
        "the relay did not follow the IPv6-only answer"
    );
    phase.store(2, std::sync::atomic::Ordering::Relaxed);
    let started = std::time::Instant::now();
    while started.elapsed() < Duration::from_secs(45) && now != Some(b'A') {
        now = udp_tag(&sock).await.or(now);
    }
    assert_eq!(
        now,
        Some(b'A'),
        "the relay stayed on IPv6 once IPv4 was listed again"
    );
}
