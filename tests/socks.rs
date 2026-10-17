mod common;

use std::net::SocketAddr;

use burrow::allowlist::{AllowList, Rule};
use burrow::config::settings::Forward;
use burrow::secret::Secret;
use common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

async fn socks_connect(proxy: SocketAddr, host: &str, port: u16) -> (u8, TcpStream) {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    s.write_all(&[5, 1, 0]).await.unwrap();
    let mut chosen = [0u8; 2];
    s.read_exact(&mut chosen).await.unwrap();
    assert_eq!(chosen, [5, 0]);
    let mut req = vec![5, 1, 0];
    match host.parse::<std::net::Ipv4Addr>() {
        Ok(ip) => {
            req.push(1);
            req.extend_from_slice(&ip.octets());
        }
        Err(_) => {
            req.push(3);
            req.push(host.len() as u8);
            req.extend_from_slice(host.as_bytes());
        }
    }
    req.extend_from_slice(&port.to_be_bytes());
    s.write_all(&req).await.unwrap();
    let mut reply = [0u8; 10];
    s.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply[0], 5);
    (reply[1], s)
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
    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
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
