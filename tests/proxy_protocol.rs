mod common;

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use burrow::config::model::Protocol;
use burrow::config::settings::Service;
use burrow::protocol::VisitTarget;
use burrow::proxy_protocol::{V2_SIGNATURE, Version};
use burrow::secret::Secret;
use common::*;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

async fn read_header(r: &mut BufReader<TcpStream>) -> (SocketAddr, SocketAddr) {
    let head = r.fill_buf().await.unwrap();
    if head.starts_with(b"PROXY ") {
        let mut line = String::new();
        r.read_line(&mut line).await.unwrap();
        let parts: Vec<&str> = line.trim_end().split(' ').collect();
        assert_eq!(parts.len(), 6, "{line:?}");
        assert!(parts[1] == "TCP4" || parts[1] == "TCP6", "{line:?}");
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        let port = |s: &str| s.parse::<u16>().unwrap();
        return (
            SocketAddr::new(ip(parts[2]), port(parts[4])),
            SocketAddr::new(ip(parts[3]), port(parts[5])),
        );
    }
    let mut fixed = [0u8; 16];
    r.read_exact(&mut fixed).await.unwrap();
    assert_eq!(fixed[..12], V2_SIGNATURE, "no PROXY header");
    assert_eq!(fixed[12], 0x21, "version 2, PROXY command");
    let len = u16::from_be_bytes([fixed[14], fixed[15]]) as usize;
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await.unwrap();
    let port = |at: usize| u16::from_be_bytes([body[at], body[at + 1]]);
    match fixed[13] {
        0x11 => {
            let ip = |at: usize| {
                IpAddr::V4(Ipv4Addr::new(
                    body[at],
                    body[at + 1],
                    body[at + 2],
                    body[at + 3],
                ))
            };
            (
                SocketAddr::new(ip(0), port(8)),
                SocketAddr::new(ip(4), port(10)),
            )
        }
        0x21 => {
            let ip = |at: usize| {
                let mut b = [0u8; 16];
                b.copy_from_slice(&body[at..at + 16]);
                IpAddr::V6(Ipv6Addr::from(b))
            };
            (
                SocketAddr::new(ip(0), port(32)),
                SocketAddr::new(ip(16), port(34)),
            )
        }
        other => panic!("unexpected family byte {other:#x}"),
    }
}

async fn proxy_aware_echo() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = l.accept().await else { return };
            tokio::spawn(async move {
                let mut r = BufReader::new(s);
                let (src, dst) = read_header(&mut r).await;
                let rest = r.buffer().to_vec();
                let mut s = r.into_inner();
                s.write_all(format!("{src} {dst}\n").as_bytes())
                    .await
                    .unwrap();
                s.write_all(&rest).await.unwrap();
                let (mut rd, mut wr) = s.split();
                let _ = tokio::io::copy(&mut rd, &mut wr).await;
                let _ = wr.shutdown().await;
            });
        }
    });
    addr
}

async fn seen_through(addr: SocketAddr) -> (SocketAddr, SocketAddr, SocketAddr) {
    let conn = TcpStream::connect(addr).await.unwrap();
    let me = conn.local_addr().unwrap();
    let mut r = BufReader::new(conn);
    let mut line = String::new();
    r.read_line(&mut line).await.unwrap();
    let (src, dst) = line.trim_end().split_once(' ').unwrap();
    let data = pattern(300_000, 5);
    let mut conn = r.into_inner();
    let (mut rd, mut wr) = conn.split();
    let send = async {
        wr.write_all(&data).await.unwrap();
        wr.shutdown().await.unwrap();
    };
    let mut back = Vec::new();
    let recv = rd.read_to_end(&mut back);
    let (_, got) = tokio::join!(send, recv);
    got.unwrap();
    assert_eq!(back, data, "data after the header arrives unchanged");
    (src.parse().unwrap(), dst.parse().unwrap(), me)
}

fn with(version: Version, target: SocketAddr) -> Service {
    Service {
        proxy_protocol: Some(version),
        ..service(target, Protocol::Tcp)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn public_services_send_the_real_source_in_v1_and_v2() {
    let target = proxy_aware_echo().await;
    let t = tunnel(vec![
        ("v1", with(Version::V1, target)),
        ("v2", with(Version::V2, target)),
    ])
    .await;
    for name in ["v1", "v2"] {
        let public = t.server.services[name];
        let (src, dst, me) = seen_through(public).await;
        assert_eq!(src, me, "{name}: the source is the real peer");
        assert_eq!(dst, public, "{name}: the destination is the public address");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_header_also_works_with_insecure_plaintext() {
    let target = proxy_aware_echo().await;
    let secret = Secret::generate();
    let mut server = server_settings(vec![secret.clone()]);
    server.insecure_plaintext = true;
    let t = tunnel_with(
        server,
        vec![
            ("v1", with(Version::V1, target)),
            ("v2", with(Version::V2, target)),
        ],
        |mut c| {
            c.insecure_plaintext = true;
            c
        },
        secret,
    )
    .await;
    for name in ["v1", "v2"] {
        let (src, _, me) = seen_through(t.server.services[name]).await;
        assert_eq!(src, me, "{name}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn visits_send_the_visitor_address() {
    let target = proxy_aware_echo().await;
    let secret = Secret::generate();
    let visitor_secret = Secret::generate();
    let mut server = server_settings(vec![secret.clone()]);
    server
        .visitors
        .insert("laptop".into(), visitor_auth(visitor_secret.clone(), &[]));
    server.services.insert(
        "db".into(),
        Service {
            proxy_protocol: Some(Version::V2),
            ..private_service(target, &["laptop"])
        },
    );
    let t = tunnel_with(server, vec![], |c| c, secret).await;
    let v = start_visitor(visitor_settings(
        t.server.addr,
        visitor_secret,
        vec![("db", VisitTarget::Service("db".into()))],
    ));
    let (src, dst, _) = seen_through(v.addr("db")).await;
    assert!(src.ip().is_loopback(), "{src}");
    assert_ne!(src.port(), 0);
    assert!(dst.ip().is_loopback(), "{dst}");
}
