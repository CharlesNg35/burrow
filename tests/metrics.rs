mod common;

use std::net::SocketAddr;
use std::time::Duration;

use burrow::allowlist::Sources;
use burrow::config::settings::{Health, Service};
use burrow::protocol::VisitTarget;
use burrow::secret::Secret;
use burrow_config::Protocol;
use common::*;
use tokio::net::TcpStream;

fn http_get(addr: SocketAddr, path: &str) -> String {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
        .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out
}

async fn metered(services: Vec<(&str, Service)>) -> Tunnel {
    let secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    settings.metrics = Some("127.0.0.1:0".parse().unwrap());
    tunnel_with(settings, services, |c| c, secret).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn metrics_are_off_unless_configured() {
    let echo = echo_server().await;
    let t = tunnel(vec![("web", service(echo, Protocol::Tcp))]).await;
    assert_eq!(t.server.metrics, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn status_counts_connections_and_bytes() {
    let echo = echo_server().await;
    let t = metered(vec![("web", service(echo, Protocol::Tcp))]).await;
    let addr = t.server.metrics.unwrap();
    for seed in 0..3 {
        let data = pattern(10_000, seed);
        assert_eq!(roundtrip(t.server.services["web"], &data).await, data);
    }
    let mut s = t.server.shared.status();
    for _ in 0..50 {
        if s.services[0].open == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        s = t.server.shared.status();
    }
    assert_eq!(s.version, env!("CARGO_PKG_VERSION"));
    let home = &s.clients[0];
    assert_eq!((home.name.as_str(), home.connected), ("home", true));
    assert!(
        home.from
            .as_deref()
            .is_some_and(|f| f.starts_with("127.0.0.1:"))
    );
    let web = &s.services[0];
    assert_eq!(web.name, "web");
    assert_eq!((web.total, web.open, web.refused), (3, 0, 0));
    assert_eq!((web.bytes_in, web.bytes_out), (30_000, 30_000));
    let text = http_get(addr, "/metrics");
    assert!(text.starts_with("HTTP/1.1 200 OK\r\n"), "{text}");
    for line in [
        "burrow_service_connections_total{service=\"web\"} 3",
        "burrow_service_bytes_total{service=\"web\",direction=\"in\"} 30000",
        "burrow_client_connected{client=\"home\"} 1",
    ] {
        assert!(text.lines().any(|l| l == line), "missing {line} in\n{text}");
    }
    assert!(http_get(addr, "/nope").starts_with("HTTP/1.1 404"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refusals_and_health_show_up() {
    let echo = echo_server().await;
    let blocked = Service {
        sources: Sources {
            allow: vec![],
            deny: vec!["127.0.0.0/8".parse().unwrap()],
        },
        ..service(echo, Protocol::Tcp)
    };
    let checked = Service {
        health: Some(Health {
            interval: Duration::from_millis(200),
            timeout: Duration::from_secs(1),
            failures: 1,
        }),
        ..service(echo, Protocol::Tcp)
    };
    let t = metered(vec![("blocked", blocked), ("checked", checked)]).await;
    let _ = try_roundtrip(t.server.services["blocked"], b"x").await;
    let mut s = t.server.shared.status();
    for _ in 0..50 {
        if !s.services[1].health.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        s = t.server.shared.status();
    }
    assert_eq!(s.services[0].name, "blocked");
    assert_eq!(s.services[0].refused, 1);
    assert_eq!(s.services[1].health.len(), 1);
    assert!(s.services[1].health[0].up);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reload_turns_the_endpoint_on_and_off() {
    let secret = Secret::generate();
    let settings = server_settings(vec![secret.clone()]);
    let t = tunnel_with(settings.clone(), vec![], |c| c, secret).await;
    let (held, port) = refusing();
    let answers = |port| async move {
        tokio::task::spawn_blocking(move || scrape(port).is_ok())
            .await
            .unwrap()
    };
    let mut on = settings.clone();
    on.metrics = Some(port);
    drop(held);
    t.server.reload(on).await;
    let mut up = false;
    for _ in 0..50 {
        if answers(port).await {
            up = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(up, "metrics must start after reload");
    assert_eq!(t.server.shared.status().clients[0].name, "home");
    t.server.reload(settings).await;
    for _ in 0..50 {
        if !answers(port).await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("metrics must stop after reload");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn visit_bytes_count_payload_only() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let visitor_secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    settings.metrics = Some("127.0.0.1:0".parse().unwrap());
    settings
        .visitors
        .insert("laptop".into(), visitor_auth(visitor_secret.clone(), &[]));
    let t = tunnel_with(
        settings,
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
    let data = pattern(10_000, 1);
    assert_eq!(roundtrip(v.addr("web"), &data).await, data);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let s = t.server.shared.status();
    let web = &s.services[0];
    assert_eq!((web.bytes_in, web.bytes_out), (10_000, 10_000));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_visitor_shows_when_it_last_used_a_tunnel() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let echo = echo_server().await;
    let secret = Secret::generate();
    let visitor_secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    settings
        .visitors
        .insert("laptop".into(), visitor_auth(visitor_secret.clone(), &[]));
    settings
        .visitors
        .insert("idle".into(), visitor_auth(Secret::generate(), &[]));
    let t = tunnel_with(
        settings,
        vec![("web", private_service(echo, &["laptop"]))],
        |c| c,
        secret,
    )
    .await;
    let laptop = |s: &burrow::metrics::Status| {
        s.visitors
            .iter()
            .find(|v| v.name == "laptop")
            .cloned()
            .unwrap()
    };
    let before = laptop(&t.server.shared.status());
    assert_eq!((before.open, before.last_seen_secs), (0, None));
    let v = start_visitor(visitor_settings(
        t.server.addr,
        visitor_secret,
        vec![("web", VisitTarget::service("web"))],
    ));
    let mut conn = TcpStream::connect(v.addr("web")).await.unwrap();
    conn.write_all(b"ping").await.unwrap();
    let mut back = [0u8; 4];
    conn.read_exact(&mut back).await.unwrap();
    let open = laptop(&t.server.shared.status());
    assert_eq!((open.open, open.last_seen_secs), (1, Some(0)));
    drop(conn);
    wait_for(|| laptop(&t.server.shared.status()).open == 0).await;
    let closed = laptop(&t.server.shared.status());
    assert!(closed.last_seen_secs.is_some_and(|s| s <= 1), "{closed:?}");
    let idle = t.server.shared.status();
    let idle = idle.visitors.iter().find(|v| v.name == "idle").unwrap();
    assert_eq!(idle.last_seen_secs, None);
    let json = serde_json::to_value(&t.server.shared.status()).unwrap();
    assert!(
        json["visitors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["name"] == "laptop" && v["last_seen_secs"].is_u64()),
        "{json}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plaintext_visit_bytes_count_payload_only() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let visitor_secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    settings.metrics = Some("127.0.0.1:0".parse().unwrap());
    settings.insecure_plaintext = true;
    settings
        .visitors
        .insert("laptop".into(), visitor_auth(visitor_secret.clone(), &[]));
    let t = tunnel_with(
        settings,
        vec![("web", private_service(echo, &["laptop"]))],
        |mut c| {
            c.insecure_plaintext = true;
            c
        },
        secret,
    )
    .await;
    let v = start_visitor(visitor_settings(
        t.server.addr,
        visitor_secret,
        vec![("web", VisitTarget::service("web"))],
    ));
    let data = pattern(10_000, 1);
    assert_eq!(roundtrip(v.addr("web"), &data).await, data);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let s = t.server.shared.status();
    let web = &s.services[0];
    assert_eq!((web.bytes_in, web.bytes_out), (10_000, 10_000));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_connections_do_not_lock_out_the_metrics_endpoint() {
    let secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    settings.metrics = Some("127.0.0.1:0".parse().unwrap());
    let t = tunnel_with(settings, vec![], |c| c, secret).await;
    let addr = t.server.metrics.unwrap();
    let mut idle = Vec::new();
    for _ in 0..40 {
        idle.push(TcpStream::connect(addr).await.unwrap());
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let r = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::task::spawn_blocking(move || scrape(addr)),
    )
    .await;
    assert!(
        matches!(r, Ok(Ok(Ok(_)))),
        "40 idle sockets made the endpoint refuse a scrape: {r:?}"
    );
    drop(idle);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn source_refusals_are_counted_in_total() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    settings.metrics = Some("127.0.0.1:0".parse().unwrap());
    let blocked = Service {
        sources: burrow::allowlist::Sources {
            allow: vec![],
            deny: vec!["127.0.0.0/8".parse().unwrap()],
        },
        ..service(echo, Protocol::Tcp)
    };
    let t = tunnel_with(settings, vec![("blocked", blocked)], |c| c, secret).await;
    let _ = try_roundtrip(t.server.services["blocked"], b"x").await;
    let s = t.server.shared.status();
    let b = &s.services[0];
    assert!(
        b.refused <= b.total,
        "refused {} > total {}",
        b.refused,
        b.total
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_kicked_by_reload_shows_when_it_left() {
    let secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    settings.metrics = Some("127.0.0.1:0".parse().unwrap());
    let t = tunnel_with(settings.clone(), vec![], |c| c, secret).await;
    let mut rotated = settings.clone();
    rotated.clients.get_mut("home").unwrap().secrets = vec![Secret::generate()];
    t.server.reload(rotated).await;
    let shared = t.server.shared.clone();
    wait_for(|| shared.session_names().is_empty()).await;
    let s = t.server.shared.status();
    assert!(!s.clients[0].connected);
    assert!(
        s.clients[0].left_secs_ago.is_some(),
        "status says home was never seen: {:?}",
        s.clients[0]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn moving_metrics_to_a_busy_port_keeps_the_old_endpoint() {
    let secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    settings.metrics = Some("127.0.0.1:0".parse().unwrap());
    let t = tunnel_with(settings.clone(), vec![], |c| c, secret).await;
    let old = t.server.metrics.unwrap();
    let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut next = settings.clone();
    let moved = busy.local_addr().unwrap();
    next.metrics = Some(moved);
    t.server.reload(next.clone()).await;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let r = tokio::task::spawn_blocking(move || scrape(old))
        .await
        .unwrap();
    assert!(
        r.is_ok(),
        "no metrics endpoint left after a failed move: {r:?}"
    );
    drop(busy);
    t.server.reload(next).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let r = tokio::task::spawn_blocking(move || scrape(moved))
        .await
        .unwrap();
    assert!(r.is_ok(), "the same reload did not retry the move: {r:?}");
}

fn raw_http(addr: SocketAddr, request: &[u8]) -> String {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let _ = s.write_all(request);
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    String::from_utf8_lossy(&out).into_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn prometheus_text_has_each_family_once_and_unique_series() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let visitor_secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    settings.metrics = Some("127.0.0.1:0".parse().unwrap());
    settings
        .visitors
        .insert("lap\"top\\x".into(), visitor_auth(visitor_secret, &[]));
    let checked = Service {
        health: Some(Health {
            interval: Duration::from_millis(200),
            timeout: Duration::from_secs(1),
            failures: 1,
        }),
        ..service(echo, Protocol::Tcp)
    };
    let t = tunnel_with(
        settings,
        vec![
            ("web", checked),
            ("api", service(echo, Protocol::Tcp)),
            ("dns", service(udp_echo_server().await, Protocol::Udp)),
        ],
        |c| c,
        secret,
    )
    .await;
    let addr = t.server.metrics.unwrap();
    let shared = t.server.shared.clone();
    wait_for(|| !shared.status().services.iter().all(|s| s.health.is_empty())).await;
    let text =
        tokio::task::spawn_blocking(move || raw_http(addr, b"GET /metrics HTTP/1.1\r\n\r\n"))
            .await
            .unwrap();
    let body = text.split("\r\n\r\n").nth(1).unwrap();
    let mut help = std::collections::BTreeMap::new();
    let mut kind = std::collections::BTreeMap::new();
    let mut series = std::collections::BTreeSet::new();
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("# HELP ") {
            *help
                .entry(rest.split(' ').next().unwrap().to_string())
                .or_insert(0) += 1;
        } else if let Some(rest) = line.strip_prefix("# TYPE ") {
            let mut p = rest.split(' ');
            let name = p.next().unwrap().to_string();
            let k = p.next().unwrap();
            assert!(k == "gauge" || k == "counter", "{line}");
            assert!(k != "counter" || name.ends_with("_total"), "{line}");
            *kind.entry(name).or_insert(0) += 1;
        } else {
            let (key, value) = line.rsplit_once(' ').unwrap();
            assert!(value.parse::<u64>().is_ok(), "{line}");
            let family = key.split('{').next().unwrap();
            assert_eq!(kind.get(family), Some(&1), "sample before its TYPE: {line}");
            assert!(series.insert(key.to_string()), "duplicate series {key}");
        }
    }
    assert!(help.values().all(|n| *n == 1) && kind.values().all(|n| *n == 1));
    assert_eq!(
        help.keys().collect::<Vec<_>>(),
        kind.keys().collect::<Vec<_>>()
    );
    assert!(series.contains("burrow_visitor_connections_total{visitor=\"lap\\\"top\\\\x\"}"));
    assert!(series.contains("burrow_service_client_up{service=\"web\",client=\"home\"}"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_metrics_endpoint_handles_odd_requests() {
    let secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    settings.metrics = Some("127.0.0.1:0".parse().unwrap());
    let t = tunnel_with(settings, vec![], |c| c, secret).await;
    let addr = t.server.metrics.unwrap();
    tokio::task::spawn_blocking(move || {
        assert!(
            raw_http(addr, b"POST /metrics HTTP/1.1\r\nContent-Length: 0\r\n\r\n")
                .starts_with("HTTP/1.1 405")
        );
        assert!(raw_http(addr, b"HEAD /metrics HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 405"));
        assert!(raw_http(addr, b"GET /metricsx HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 404"));
        let mut huge = b"GET /metrics HTTP/1.1\r\nX: ".to_vec();
        huge.extend(std::iter::repeat_n(b'a', 64 * 1024));
        assert_eq!(raw_http(addr, &huge), "");
        assert_eq!(raw_http(addr, b""), "");
        assert!(raw_http(addr, b"GET /status HTTP/1.0\r\n\r\n").starts_with("HTTP/1.0 404"));
        assert!(scrape(addr).is_ok());
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_udp_peer_counts_as_one_connection() {
    let echo = udp_echo_server().await;
    let t = tunnel(vec![("dns", service(echo, Protocol::Udp))]).await;
    let target = t.server.services["dns"];
    let mut buf = vec![0u8; 65536];
    for peer in 0..3 {
        let s = udp_bind().await;
        s.connect(target).await.unwrap();
        for round in 0..5 {
            let msg = format!("peer {peer} round {round}");
            let mut got = false;
            for _ in 0..50 {
                s.send(msg.as_bytes()).await.unwrap();
                if let Ok(Ok(n)) =
                    tokio::time::timeout(Duration::from_millis(200), s.recv(&mut buf)).await
                    && &buf[..n] == msg.as_bytes()
                {
                    got = true;
                    break;
                }
            }
            assert!(got, "no echo for {msg}");
        }
    }
    let dns = t.server.shared.status().services[0].clone();
    assert_eq!(dns.total, 3);
    assert_eq!(t.server.shared.status().udp_peers, 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reload_that_moves_metrics_onto_the_kept_listen_port_is_rejected() {
    if ws_mode() {
        return;
    }
    let secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    let (held, kept) = refusing();
    drop(held);
    settings.listen = Some(kept);
    let t = tunnel_with(settings.clone(), vec![], |c| c, secret).await;
    let (held, other) = refusing();
    drop(held);
    let mut next = settings.clone();
    next.listen = Some(other);
    next.metrics = Some(kept);
    next.max_handshakes = settings.max_handshakes + 1;
    t.server.reload(next).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let now = t.server.shared.settings_snapshot();
    assert_eq!(now.max_handshakes, settings.max_handshakes);
    assert_eq!(now.metrics, None);
}
