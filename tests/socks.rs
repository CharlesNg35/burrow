mod common;

use std::net::SocketAddr;
use std::time::Duration;

use burrow::allowlist::{AllowList, Rule};
use burrow::config::settings::Forward;
use burrow::secret::Secret;
use common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn connect_request(host: &str, port: u16) -> Vec<u8> {
    let mut req = vec![5, 1, 0];
    if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
        req.push(1);
        req.extend_from_slice(&ip.octets());
    } else if let Ok(ip) = host.parse::<std::net::Ipv6Addr>() {
        req.push(4);
        req.extend_from_slice(&ip.octets());
    } else {
        req.push(3);
        req.push(host.len() as u8);
        req.extend_from_slice(host.as_bytes());
    }
    req.extend_from_slice(&port.to_be_bytes());
    req
}

async fn greet(proxy: SocketAddr) -> TcpStream {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    s.write_all(&[5, 1, 0]).await.unwrap();
    let mut chosen = [0u8; 2];
    s.read_exact(&mut chosen).await.unwrap();
    assert_eq!(chosen, [5, 0]);
    s
}

async fn reply_code(s: &mut TcpStream) -> u8 {
    let mut reply = [0u8; 10];
    tokio::time::timeout(Duration::from_secs(10), s.read_exact(&mut reply))
        .await
        .expect("a SOCKS reply in time")
        .unwrap();
    assert_eq!(reply[0], 5);
    reply[1]
}

async fn socks_connect(proxy: SocketAddr, host: &str, port: u16) -> (u8, TcpStream) {
    let mut s = greet(proxy).await;
    s.write_all(&connect_request(host, port)).await.unwrap();
    let code = reply_code(&mut s).await;
    (code, s)
}

async fn echoes(mut s: TcpStream, data: &[u8]) {
    let (mut r, mut w) = s.split();
    let send = async {
        w.write_all(data).await.unwrap();
        w.shutdown().await.unwrap();
    };
    let mut back = Vec::new();
    let (_, got) = tokio::join!(send, r.read_to_end(&mut back));
    got.unwrap();
    assert_eq!(back, data);
}

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
    _tunnel: Tunnel,
    _visitor: RunningVisitor,
    addr: SocketAddr,
}

async fn proxy(reach: &[&str], allow: Vec<&str>) -> Proxy {
    let secret = Secret::generate();
    let visitor_secret = Secret::generate();
    let mut server = server_settings(vec![secret.clone()]);
    server
        .visitors
        .insert("laptop".into(), visitor_auth(visitor_secret.clone(), reach));
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
        Forward::socks("127.0.0.1:0".parse().unwrap(), "home".into()),
    );
    let visitor = start_visitor(settings);
    Proxy {
        addr: visitor.addr("home"),
        _visitor: visitor,
        _tunnel: tunnel,
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
    let (code, _) = socks_connect(v.addr("home"), "127.0.0.1", 22).await;
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
