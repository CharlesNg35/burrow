mod common;

use std::time::Duration;

use burrow::allowlist::{Cidr, Sources};
use burrow::config::settings::Service;
use burrow::protocol::VisitTarget;
use burrow::secret::Secret;
use burrow_config::Protocol;
use common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
    let mut refused = false;
    for _ in 0..50 {
        if !reaches(web_addr).await {
            refused = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(refused, "new connections must be refused after the reload");
    assert_cut(cut).await;
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
    let s = udp_bind().await;
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

fn deny_local(base: Service) -> Service {
    Service {
        sources: Sources {
            allow: vec![],
            deny: vec!["127.0.0.0/8".parse::<Cidr>().unwrap()],
        },
        ..base
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_connection_opened_while_a_slow_reload_is_applied_is_still_filtered() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let settings = server_settings(vec![secret.clone()]);
    let web = service(echo, Protocol::Tcp);
    let t = tunnel_with(settings.clone(), vec![("web", web.clone())], |c| c, secret).await;
    let web_addr = t.server.services["web"];
    let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut next = settings;
    next.services.insert(
        "aaa".into(),
        Service {
            bind: Some(busy.local_addr().unwrap()),
            ..web.clone()
        },
    );
    next.services.insert("web".into(), deny_local(web));
    t.server.reload(next).await;
    let shared = t.server.shared.clone();
    wait_for(|| !shared.settings_snapshot().services["web"].sources.is_open()).await;
    let mut conn = tokio::net::TcpStream::connect(web_addr).await.unwrap();
    let mut refused = false;
    for _ in 0..100 {
        if try_roundtrip(web_addr, b"x").await.as_deref() != Some(b"x") {
            refused = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        refused,
        "new connections are refused once the reload is done"
    );
    let still = tokio::time::timeout(Duration::from_secs(2), async {
        conn.write_all(b"late").await.ok()?;
        let mut buf = [0u8; 4];
        conn.read_exact(&mut buf).await.ok()?;
        Some(buf)
    })
    .await
    .ok()
    .flatten();
    assert_eq!(
        still, None,
        "a connection from 127.0.0.1 kept working after deny_from 127.0.0.0/8 was applied"
    );
}

fn with_sources(base: Service, allow: &[&str], deny: &[&str]) -> Service {
    let parse = |l: &[&str]| l.iter().map(|s| s.parse::<Cidr>().unwrap()).collect();
    Service {
        sources: Sources {
            allow: parse(allow),
            deny: parse(deny),
        },
        ..base
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ipv4_visitors_of_a_dual_stack_bind_match_ipv4_rules() {
    if std::net::TcpListener::bind("[::1]:0").is_err() {
        return;
    }
    let echo = echo_server().await;
    let dual = Service {
        bind: Some("[::]:0".parse().unwrap()),
        ..service(echo, Protocol::Tcp)
    };
    let t = tunnel(vec![
        ("v4ok", with_sources(dual.clone(), &["127.0.0.0/8"], &[])),
        (
            "mappeddeny",
            with_sources(dual.clone(), &[], &["::ffff:127.0.0.1"]),
        ),
        ("v6only", with_sources(dual, &["::1"], &[])),
    ])
    .await;
    let at = |host: &str, name: &str| -> std::net::SocketAddr {
        format!("{host}:{}", t.server.services[name].port())
            .parse()
            .unwrap()
    };
    let ok = |a| async move { try_roundtrip(a, b"hi").await.as_deref() == Some(&b"hi"[..]) };
    assert!(ok(at("127.0.0.1", "v4ok")).await);
    assert!(!ok(at("[::1]", "v4ok")).await);
    assert!(!ok(at("127.0.0.1", "mappeddeny")).await);
    assert!(ok(at("[::1]", "mappeddeny")).await);
    assert!(ok(at("[::1]", "v6only")).await);
    assert!(!ok(at("127.0.0.1", "v6only")).await);
}

async fn still_echoes(s: &tokio::net::UdpSocket) -> bool {
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
async fn reload_stops_udp_peers_no_longer_allowed_and_readmits_them() {
    let echo = udp_echo_server().await;
    let secret = Secret::generate();
    let settings = server_settings(vec![secret.clone()]);
    let dns = service(echo, Protocol::Udp);
    let t = tunnel_with(settings.clone(), vec![("dns", dns.clone())], |c| c, secret).await;
    let s = udp_bind().await;
    s.connect(t.server.services["dns"]).await.unwrap();
    assert!(still_echoes(&s).await);
    let mut next = settings;
    next.services
        .insert("dns".into(), with_sources(dns.clone(), &[], &["127.0.0.1"]));
    t.server.reload(next.clone()).await;
    let shared = t.server.shared.clone();
    wait_for(|| !shared.settings_snapshot().services["dns"].sources.is_open()).await;
    let mut stale = [0u8; 64];
    while tokio::time::timeout(Duration::from_millis(200), s.recv(&mut stale))
        .await
        .is_ok()
    {}
    assert!(!still_echoes(&s).await, "a denied peer is dropped at once");
    next.services
        .insert("dns".into(), with_sources(dns, &["127.0.0.0/8"], &[]));
    t.server.reload(next).await;
    let mut back = false;
    for _ in 0..20 {
        if still_echoes(&s).await {
            back = true;
            break;
        }
    }
    assert!(back, "the peer is admitted again");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reload_closes_a_visit_from_a_source_no_longer_allowed() {
    use burrow::protocol::VisitTarget;
    let echo = echo_server().await;
    let secret = Secret::generate();
    let visitor_secret = Secret::generate();
    let mut server = server_settings(vec![secret.clone()]);
    server
        .visitors
        .insert("laptop".into(), visitor_auth(visitor_secret.clone(), &[]));
    let db = private_service(echo, &["laptop"]);
    server.services.insert("db".into(), db.clone());
    let t = tunnel_with(server.clone(), vec![], |c| c, secret).await;
    let v = start_visitor(visitor_settings(
        t.server.addr,
        visitor_secret,
        vec![("db", VisitTarget::service("db"))],
    ));
    let conn = open_echo(v.addr("db")).await;
    let mut next = server;
    next.services
        .insert("db".into(), with_sources(db, &[], &["127.0.0.0/8"]));
    t.server.reload(next).await;
    assert_cut(conn).await;
    assert!(try_roundtrip(v.addr("db"), b"hi").await.as_deref() != Some(&b"hi"[..]));
}
