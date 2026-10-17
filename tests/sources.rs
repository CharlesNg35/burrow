mod common;

use std::time::Duration;

use burrow::allowlist::{Cidr, Sources};
use burrow::config::model::Protocol;
use burrow::config::settings::Service;
use burrow::protocol::VisitTarget;
use burrow::secret::Secret;
use common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;

fn nets(list: &[&str]) -> Vec<Cidr> {
    list.iter().map(|s| s.parse().unwrap()).collect()
}

fn filtered(base: Service, allow: &[&str], deny: &[&str]) -> Service {
    Service {
        sources: Sources {
            allow: nets(allow),
            deny: nets(deny),
        },
        ..base
    }
}

async fn reaches(addr: std::net::SocketAddr) -> bool {
    let payload = pattern(2000, 9);
    try_roundtrip(addr, &payload).await.as_deref() == Some(&payload[..])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tcp_services_admit_only_permitted_sources() {
    let echo = echo_server().await;
    let open = service(echo, Protocol::Tcp);
    let t = tunnel(vec![
        ("open", open.clone()),
        ("local", filtered(open.clone(), &["127.0.0.0/8"], &[])),
        ("office", filtered(open.clone(), &["203.0.113.0/24"], &[])),
        ("blocked", filtered(open.clone(), &[], &["127.0.0.1"])),
        (
            "carved",
            filtered(open, &["127.0.0.0/8", "::1"], &["127.0.0.1"]),
        ),
    ])
    .await;
    let s = &t.server.services;
    assert!(reaches(s["open"]).await);
    assert!(reaches(s["local"]).await);
    assert!(!reaches(s["office"]).await);
    assert!(!reaches(s["blocked"]).await);
    assert!(!reaches(s["carved"]).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reload_closes_connections_from_sources_no_longer_allowed() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let settings = server_settings(vec![secret.clone()]);
    let web = service(echo, Protocol::Tcp);
    let t = tunnel_with(
        settings.clone(),
        vec![("web", web.clone()), ("other", web.clone())],
        |c| c,
        secret,
    )
    .await;
    let web_addr = t.server.services["web"];
    let mut kept = open_echo(t.server.services["other"]).await;
    let cut = open_echo(web_addr).await;
    let mut next = settings;
    next.services.insert("other".into(), web.clone());
    next.services
        .insert("web".into(), filtered(web.clone(), &[], &["127.0.0.0/8"]));
    t.server.reload(next.clone()).await;
    assert_cut(cut).await;
    assert!(!reaches(web_addr).await);
    kept.write_all(b"ping").await.unwrap();
    let mut buf = [0u8; 4];
    kept.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"ping", "other services keep their connections");
    next.services
        .insert("web".into(), filtered(web, &["127.0.0.1"], &[]));
    t.server.reload(next).await;
    for _ in 0..50 {
        if reaches(web_addr).await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("an allowed source must reach the service again after reload");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn visitors_of_a_private_service_are_filtered_by_source_too() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let visitor_secret = Secret::generate();
    let mut server = server_settings(vec![secret.clone()]);
    server
        .visitors
        .insert("laptop".into(), visitor_auth(visitor_secret.clone(), &[]));
    let db = private_service(echo, &["laptop"]);
    server
        .services
        .insert("db".into(), filtered(db.clone(), &["127.0.0.0/8"], &[]));
    server
        .services
        .insert("office".into(), filtered(db, &["203.0.113.0/24"], &[]));
    let t = tunnel_with(server, vec![], |c| c, secret).await;
    let v = start_visitor(visitor_settings(
        t.server.addr,
        visitor_secret,
        vec![
            ("db", VisitTarget::service("db")),
            ("office", VisitTarget::service("office")),
        ],
    ));
    assert!(reaches(v.addr("db")).await);
    assert!(!reaches(v.addr("office")).await);
}

async fn udp_echoes(target: std::net::SocketAddr) -> bool {
    let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    s.connect(target).await.unwrap();
    let mut buf = [0u8; 64];
    for _ in 0..15 {
        s.send(b"hello").await.unwrap();
        if let Ok(Ok(n)) = tokio::time::timeout(Duration::from_millis(150), s.recv(&mut buf)).await
            && &buf[..n] == b"hello"
        {
            return true;
        }
    }
    false
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn udp_datagrams_from_other_sources_are_dropped() {
    let echo = udp_echo_server().await;
    let dns = service(echo, Protocol::Udp);
    let t = tunnel(vec![
        ("allowed", filtered(dns.clone(), &["127.0.0.0/8"], &[])),
        ("denied", filtered(dns, &[], &["127.0.0.0/8"])),
    ])
    .await;
    assert!(udp_echoes(t.server.services["allowed"]).await);
    assert!(!udp_echoes(t.server.services["denied"]).await);
}
