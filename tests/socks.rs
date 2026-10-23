mod common;

use std::net::SocketAddr;
use std::time::Duration;

use burrow::allowlist::{AllowList, Rule};
use burrow::config::settings::Forward;
use burrow::secret::Secret;
use common::socks::*;
use common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn echo_on(addr: &str) -> SocketAddr {
    let listener = TcpListener::bind(addr).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
                let _ = w.shutdown().await;
            });
        }
    });
    addr
}

struct Proxy {
    tunnel: Tunnel,
    _visitor: RunningVisitor,
    addr: SocketAddr,
}

async fn proxy(reach: &[&str], allow: Vec<&str>) -> Proxy {
    proxy_with(reach, allow, |_| {}).await
}

async fn proxy_with(
    reach: &[&str],
    allow: Vec<&str>,
    edit: impl FnOnce(&mut burrow::config::settings::ServerSettings),
) -> Proxy {
    proxy_at(reach, allow, edit, "127.0.0.1:0").await
}

async fn proxy_at(
    reach: &[&str],
    allow: Vec<&str>,
    edit: impl FnOnce(&mut burrow::config::settings::ServerSettings),
    local: &str,
) -> Proxy {
    let secret = Secret::generate();
    let visitor_secret = Secret::generate();
    let mut server = server_settings(vec![secret.clone()]);
    server
        .visitors
        .insert("laptop".into(), visitor_auth(visitor_secret.clone(), reach));
    edit(&mut server);
    let rules: Vec<Rule> = allow.iter().map(|r| r.parse().unwrap()).collect();
    let tunnel = tunnel_with(
        server,
        vec![],
        move |mut c| {
            c.allow = AllowList::Rules(rules);
            c
        },
        secret,
    )
    .await;
    let mut settings = visitor_settings(tunnel.server.addr, visitor_secret, vec![]);
    settings.forwards.insert(
        "home".into(),
        Forward::socks(local.parse().unwrap(), "home".into()),
    );
    let visitor = start_visitor(settings);
    Proxy {
        addr: visitor.addr("home"),
        _visitor: visitor,
        tunnel,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ipv6_targets_are_reached() {
    let Ok(probe) = std::net::TcpListener::bind("[::1]:0") else {
        return;
    };
    drop(probe);
    let echo = echo_on("[::1]:0").await;
    let p = proxy(&["home"], vec!["[::1/128]"]).await;
    let (code, s) = socks_connect(p.addr, "::1", echo.port()).await;
    assert_eq!(code, 0);
    echoes(s, &pattern(30_000, 3)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn data_sent_right_after_the_request_is_not_lost() {
    let echo = echo_server().await;
    let p = proxy(&["home"], vec!["127.0.0.0/8"]).await;
    let mut s = TcpStream::connect(p.addr).await.unwrap();
    let mut all = vec![5, 1, 0];
    all.extend_from_slice(&connect_request("127.0.0.1", echo.port()));
    all.extend_from_slice(b"early bytes");
    s.write_all(&all).await.unwrap();
    let mut chosen = [0u8; 2];
    s.read_exact(&mut chosen).await.unwrap();
    assert_eq!(chosen, [5, 0]);
    assert_eq!(reply_code(&mut s).await, 0);
    let mut back = [0u8; 11];
    s.read_exact(&mut back).await.unwrap();
    assert_eq!(&back, b"early bytes");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bad_and_silent_clients_do_not_disturb_good_ones() {
    let echo = echo_server().await;
    let p = proxy(&["home"], vec!["127.0.0.0/8"]).await;
    let mut silent = Vec::new();
    for _ in 0..20 {
        let mut s = TcpStream::connect(p.addr).await.unwrap();
        s.write_all(&[5]).await.unwrap();
        silent.push(s);
    }
    for garbage in [
        &b"GET / HTTP/1.1\r\n\r\n"[..],
        &[5, 0][..],
        &[4, 1, 0, 80, 127, 0, 0, 1, 0][..],
        &[5, 1, 0, 5, 1, 0, 9][..],
        &[5, 1, 0, 5, 1, 0, 3, 0, 0, 80][..],
        &[5, 1, 0, 5, 1, 0, 3, 2, 0xff, 0xfe, 0, 80][..],
        &[5, 1, 0, 6, 1, 0, 1, 127, 0, 0, 1, 0, 80][..],
    ] {
        let mut s = TcpStream::connect(p.addr).await.unwrap();
        s.write_all(garbage).await.unwrap();
        let mut sink = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut sink))
            .await
            .expect("the proxy answers or closes");
        assert!(sink.len() <= 12, "{sink:?}");
    }
    let (code, s) = socks_connect(p.addr, "127.0.0.1", echo.port()).await;
    assert_eq!(code, 0);
    echoes(s, b"still works").await;
    drop(silent);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn names_too_long_for_a_target_are_refused_with_a_reply() {
    let echo = echo_server().await;
    let p = proxy(&["home"], vec!["127.0.0.0/8"]).await;
    let long = format!("{}.example", "a".repeat(247));
    assert_eq!(long.len(), 255);
    let (code, _) = socks_connect(p.addr, &long, echo.port()).await;
    assert_ne!(code, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn many_socks_connections_at_once() {
    let echo = echo_server().await;
    let p = proxy(&["home"], vec!["127.0.0.0/8"]).await;
    let mut tasks = tokio::task::JoinSet::new();
    for i in 0..100u64 {
        let addr = p.addr;
        let port = echo.port();
        tasks.spawn(async move {
            let (code, s) = socks_connect(addr, "127.0.0.1", port).await;
            assert_eq!(code, 0);
            echoes(s, &pattern(4_000, i)).await;
        });
    }
    while let Some(r) = tasks.join_next().await {
        r.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unreachable_server_answers_network_unreachable() {
    let (_held, gone) = refusing();
    let mut settings = visitor_settings(gone, Secret::generate(), vec![]);
    settings.forwards.insert(
        "home".into(),
        Forward::socks("127.0.0.1:0".parse().unwrap(), "home".into()),
    );
    let v = start_visitor(settings);
    let (code, _) = tokio::time::timeout(
        Duration::from_secs(4),
        socks_connect(v.addr("home"), "127.0.0.1", 22),
    )
    .await
    .expect("a server that is down is reported at once, not retried");
    assert_eq!(code, 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn many_plain_visits_at_once() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let visitor_secret = Secret::generate();
    let mut server = server_settings(vec![secret.clone()]);
    server.visitors.insert(
        "laptop".into(),
        visitor_auth(visitor_secret.clone(), &["home"]),
    );
    let tunnel = tunnel_with(server, vec![], |c| c, secret).await;
    let to = burrow::protocol::VisitTarget::Client {
        client: "home".into(),
        target: echo.to_string(),
    };
    let v = start_visitor(visitor_settings(
        tunnel.server.addr,
        visitor_secret,
        vec![("echo", to)],
    ));
    let addr = v.addr("echo");
    let mut tasks = tokio::task::JoinSet::new();
    for i in 0..100u64 {
        tasks.spawn(async move {
            let s = TcpStream::connect(addr).await.unwrap();
            echoes(s, &pattern(4_000, i)).await;
        });
    }
    while let Some(r) = tasks.join_next().await {
        r.unwrap();
    }
    drop(tunnel);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_burst_wider_than_the_server_handshake_limit_still_gets_through() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let visitor_secret = Secret::generate();
    let mut server = server_settings(vec![secret.clone()]);
    server.max_handshakes = 2;
    server.visitors.insert(
        "laptop".into(),
        visitor_auth(visitor_secret.clone(), &["home"]),
    );
    let tunnel = tunnel_with(server, vec![], |c| c, secret).await;
    let mut settings = visitor_settings(tunnel.server.addr, visitor_secret, vec![]);
    settings.forwards.insert(
        "home".into(),
        Forward::socks("127.0.0.1:0".parse().unwrap(), "home".into()),
    );
    let v = start_visitor(settings);
    let addr = v.addr("home");
    let port = echo.port();
    let mut tasks = tokio::task::JoinSet::new();
    for i in 0..40u64 {
        tasks.spawn(async move {
            let (code, s) = socks_connect(addr, "127.0.0.1", port).await;
            assert_eq!(code, 0);
            echoes(s, &pattern(2_000, i)).await;
        });
    }
    while let Some(r) = tasks.join_next().await {
        r.unwrap();
    }
    assert!(
        tunnel.server.shared.status().handshakes_refused > 0,
        "the burst should have hit the limit"
    );
}

#[test]
fn long_targets_survive_the_wire() {
    let host = "a".repeat(252);
    let to = burrow::protocol::VisitTarget::Client {
        client: "home".into(),
        target: format!("{host}:8080"),
    };
    let req = burrow::protocol::VisitRequest {
        check: false,
        protocol: burrow_config::Protocol::Tcp,
        to: to.clone(),
    };
    let bytes = std::panic::catch_unwind(|| req.encode());
    assert!(bytes.is_ok(), "encoding panicked");
    if let Ok(b) = bytes.unwrap() {
        let back = burrow::protocol::VisitRequest::decode(&b).unwrap();
        assert_eq!(back.to, to, "the target changed on the wire");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_socks_port_reaches_any_allowed_target() {
    let a = echo_server().await;
    let b = echo_server().await;
    let p = proxy(&["home"], vec!["127.0.0.0/8"]).await;
    let (code, s) = socks_connect(p.addr, "127.0.0.1", a.port()).await;
    assert_eq!(code, 0);
    echoes(s, &pattern(200_000, 1)).await;
    let (code, s) = socks_connect(p.addr, "localhost", b.port()).await;
    assert_eq!(code, 0, "host names are resolved on the client");
    echoes(s, &pattern(50_000, 2)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refusals_come_back_as_socks_reply_codes() {
    let echo = echo_server().await;
    let (_held, closed) = refusing();
    let closed = closed.port();
    let p = proxy(&["home"], vec![&format!("127.0.0.1/32:{}", echo.port())]).await;
    let (code, _) = socks_connect(p.addr, "127.0.0.1", closed).await;
    assert_eq!(code, 2, "outside the client's allow list");
    let allowed = proxy(&["home"], vec!["127.0.0.0/8"]).await;
    let (code, _) = socks_connect(allowed.addr, "127.0.0.1", closed).await;
    assert_eq!(code, 5, "nothing listens there");
    let no_reach = proxy(&[], vec!["127.0.0.0/8"]).await;
    let (code, _) = socks_connect(no_reach.addr, "127.0.0.1", echo.port()).await;
    assert_eq!(code, 2, "the visitor has no reach to this client");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn narrowed_reach_is_checked_after_the_client_resolves_a_name() {
    let echo = echo_server().await;
    let narrowed = |rules: &'static [&'static str]| {
        move |s: &mut burrow::config::settings::ServerSettings| {
            s.visitors.get_mut("laptop").unwrap().reach = [(
                "home".to_string(),
                AllowList::Rules(rules.iter().map(|r| r.parse().unwrap()).collect()),
            )]
            .into();
        }
    };
    let inside = proxy_with(
        &["home"],
        vec!["127.0.0.0/8", "::1/128"],
        narrowed(&["127.0.0.0/8"]),
    )
    .await;
    let (code, s) = socks_connect(inside.addr, "localhost", echo.port()).await;
    assert_eq!(code, 0);
    echoes(s, &pattern(10_000, 3)).await;
    let outside = proxy_with(
        &["home"],
        vec!["127.0.0.0/8", "::1/128"],
        narrowed(&["10.0.0.0/8"]),
    )
    .await;
    let (code, _) = socks_connect(outside.addr, "localhost", echo.port()).await;
    assert_ne!(code, 0, "a name that resolves outside reach got through");
    let (code, _) = socks_connect(outside.addr, "127.0.0.1", echo.port()).await;
    assert_ne!(code, 0, "an address outside reach got through");
}

async fn associate_from(proxy: SocketAddr, hint: SocketAddr) -> (TcpStream, SocketAddr) {
    let mut s = greet(proxy).await;
    let mut req = vec![5, 3, 0];
    match hint {
        SocketAddr::V4(a) => {
            req.push(1);
            req.extend_from_slice(&a.ip().octets());
        }
        SocketAddr::V6(a) => {
            req.push(4);
            req.extend_from_slice(&a.ip().octets());
        }
    }
    req.extend_from_slice(&hint.port().to_be_bytes());
    s.write_all(&req).await.unwrap();
    let mut reply = [0u8; 10];
    tokio::time::timeout(Duration::from_secs(10), s.read_exact(&mut reply))
        .await
        .expect("a SOCKS reply in time")
        .unwrap();
    assert_eq!(reply[..4], [5, 0, 0, 1], "UDP ASSOCIATE succeeds over IPv4");
    let relay = SocketAddr::from((
        [reply[4], reply[5], reply[6], reply[7]],
        u16::from_be_bytes([reply[8], reply[9]]),
    ));
    (s, relay)
}

async fn associate(proxy: SocketAddr) -> (TcpStream, SocketAddr) {
    associate_from(proxy, "0.0.0.0:0".parse().unwrap()).await
}

fn wrapped(frag: u8, to: SocketAddr, data: &[u8]) -> Vec<u8> {
    let SocketAddr::V4(to) = to else {
        panic!("IPv4 targets in these tests")
    };
    let mut d = vec![0, 0, frag, 1];
    d.extend_from_slice(&to.ip().octets());
    d.extend_from_slice(&to.port().to_be_bytes());
    d.extend_from_slice(data);
    d
}

fn named(host: &str, port: u16, data: &[u8]) -> Vec<u8> {
    let mut d = vec![0, 0, 0, 3, host.len() as u8];
    d.extend_from_slice(host.as_bytes());
    d.extend_from_slice(&port.to_be_bytes());
    d.extend_from_slice(data);
    d
}

async fn answer(
    app: &tokio::net::UdpSocket,
    relay: SocketAddr,
    datagram: &[u8],
) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; 65536];
    for _ in 0..10 {
        app.send_to(datagram, relay).await.unwrap();
        if let Ok(Ok((n, from))) =
            tokio::time::timeout(Duration::from_millis(500), app.recv_from(&mut buf)).await
        {
            assert_eq!(from, relay, "replies come from the relay");
            return Some(buf[..n].to_vec());
        }
    }
    None
}

async fn dns_server() -> SocketAddr {
    let s = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let at = s.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 512];
        while let Ok((n, from)) = s.recv_from(&mut buf).await {
            let q = &buf[..n];
            if n < 12 {
                continue;
            }
            let mut r = q[..2].to_vec();
            r.extend_from_slice(&[0x81, 0x80, 0, 1, 0, 1, 0, 0, 0, 0]);
            r.extend_from_slice(&q[12..]);
            r.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 127, 0, 0, 42]);
            let _ = s.send_to(&r, from).await;
        }
    });
    at
}

fn dns_query(id: u16, name: &str) -> Vec<u8> {
    let mut q = id.to_be_bytes().to_vec();
    q.extend_from_slice(&[1, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
    for label in name.split('.') {
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.extend_from_slice(&[0, 0, 1, 0, 1]);
    q
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dns_query_goes_through_udp_associate() {
    let dns = dns_server().await;
    let p = proxy(&["home"], vec!["127.0.0.0/8"]).await;
    let (_control, relay) = associate(p.addr).await;
    let app = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let query = dns_query(0x4242, "burrow.test");
    let got = answer(&app, relay, &wrapped(0, dns, &query))
        .await
        .expect("a DNS answer");
    let head = wrapped(0, dns, &[]);
    assert_eq!(
        &got[..head.len()],
        &head[..],
        "the reply names the server it came from"
    );
    let reply = &got[head.len()..];
    assert_eq!(&reply[..2], &[0x42, 0x42], "the answer to this query");
    assert_eq!(&reply[6..8], &[0, 1], "one answer");
    assert_eq!(&reply[reply.len() - 4..], &[127, 0, 0, 42]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_named_destination_is_resolved_by_the_client_and_named_in_replies() {
    let echo = udp_echo_server().await;
    let p = proxy(&["home"], vec!["127.0.0.0/8"]).await;
    let (_control, relay) = associate(p.addr).await;
    let app = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let out = named("localhost", echo.port(), b"by name");
    assert_eq!(answer(&app, relay, &out).await.unwrap(), out);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn datagrams_from_any_other_source_are_dropped() {
    let echo = udp_echo_server().await;
    let p = proxy(&["home"], vec!["127.0.0.0/8"]).await;
    let app = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let stranger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let (_control, relay) = associate_from(p.addr, app.local_addr().unwrap()).await;
    let out = wrapped(0, echo, b"stranger first");
    assert_eq!(
        answer(&stranger, relay, &out).await,
        None,
        "a source other than the one named in the request gets nothing"
    );
    let out = wrapped(0, echo, b"the app");
    assert_eq!(answer(&app, relay, &out).await.unwrap(), out);
    assert_eq!(answer(&stranger, relay, &out).await, None);

    let (_open, any) = associate(p.addr).await;
    let first = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let out = wrapped(0, echo, b"first sender");
    assert_eq!(answer(&first, any, &out).await.unwrap(), out);
    assert_eq!(
        answer(&stranger, any, &out).await,
        None,
        "without a named source, the first sender is the only one"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fragments_are_dropped() {
    let echo = udp_echo_server().await;
    let p = proxy(&["home"], vec!["127.0.0.0/8"]).await;
    let (_control, relay) = associate(p.addr).await;
    let app = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    assert_eq!(answer(&app, relay, &wrapped(1, echo, b"part")).await, None);
    assert_eq!(
        answer(&app, relay, &wrapped(0x81, echo, b"last")).await,
        None
    );
    let whole = wrapped(0, echo, b"whole");
    assert_eq!(answer(&app, relay, &whole).await.unwrap(), whole);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_association_ends_with_its_tcp_connection() {
    let echo = udp_echo_server().await;
    let p = proxy(&["home"], vec!["127.0.0.0/8"]).await;
    let (control, relay) = associate(p.addr).await;
    let app = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let out = wrapped(0, echo, b"while open");
    assert_eq!(answer(&app, relay, &out).await.unwrap(), out);
    let shared = p.tunnel.server.shared.clone();
    assert_eq!(shared.status().visitors[0].open, 1);
    drop(control);
    wait_for(|| shared.status().visitors[0].open == 0).await;
    assert_eq!(
        answer(&app, relay, &out).await,
        None,
        "nothing is relayed after the TCP connection closed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn udp_through_socks_stays_inside_reach_and_the_client_allow_list() {
    let echo = udp_echo_server().await;
    let p = proxy_with(&[], vec!["127.0.0.0/8"], |s| {
        s.visitors.get_mut("laptop").unwrap().reach = [(
            "home".to_string(),
            AllowList::Rules(vec![
                format!("127.0.0.1/32:{}", echo.port()).parse().unwrap(),
            ]),
        )]
        .into();
    })
    .await;
    let (_control, relay) = associate(p.addr).await;
    let app = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let inside = wrapped(0, echo, b"inside");
    assert_eq!(answer(&app, relay, &inside).await.unwrap(), inside);
    let other = udp_echo_server().await;
    assert_eq!(
        answer(&app, relay, &wrapped(0, other, b"outside")).await,
        None
    );
    assert_eq!(
        answer(&app, relay, &inside).await.unwrap(),
        inside,
        "the association keeps working"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ipv4_app_gets_an_ipv4_relay_from_a_dual_stack_socks_forward() {
    if std::net::TcpListener::bind("[::]:0").is_err() {
        return;
    }
    let echo = udp_echo_server().await;
    let p = proxy_at(&["home"], vec!["127.0.0.0/8"], |_| {}, "[::]:0").await;
    let at = SocketAddr::from(([127, 0, 0, 1], p.addr.port()));
    let (_control, relay) = associate(at).await;
    assert_eq!(relay.ip(), std::net::Ipv4Addr::LOCALHOST);
    let app = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let out = wrapped(0, echo, b"dual stack");
    assert_eq!(answer(&app, relay, &out).await.unwrap(), out);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_named_source_on_another_host_is_never_admitted() {
    let echo = udp_echo_server().await;
    let p = proxy(&["home"], vec!["127.0.0.0/8"]).await;
    let app = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = app.local_addr().unwrap().port();
    let (_control, relay) = associate_from(p.addr, SocketAddr::from(([127, 0, 0, 2], port))).await;
    let out = wrapped(0, echo, b"from the app's own address");
    assert_eq!(
        answer(&app, relay, &out).await.unwrap(),
        out,
        "the app's own address with the named port is admitted"
    );
    let other = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    assert_eq!(answer(&other, relay, &out).await, None);
}
