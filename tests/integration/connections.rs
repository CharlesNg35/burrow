use std::time::Duration;

use burrow::client::State;
use burrow::wire::secret::Secret;
use burrow_config::Protocol;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::common::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn udp_one_way_traffic_keeps_replies_flowing_past_the_timeout() {
    let target = udp_bind().await;
    let echo = target.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 64];
        let mut n = 0u32;
        while let Ok((len, from)) = target.recv_from(&mut buf).await {
            n += 1;
            if n.is_multiple_of(15) {
                let _ = target.send_to(&buf[..len], from).await;
            }
        }
    });
    let secret = Secret::generate();
    let mut s = server_settings(vec![secret.clone()]);
    s.udp_session_timeout = Duration::from_secs(1);
    let t = tunnel_with(
        s,
        vec![("dns", service(echo, Protocol::Udp))],
        |mut c| {
            c.udp_session_timeout = Duration::from_secs(1);
            c
        },
        secret,
    )
    .await;
    let sock = udp_bind().await;
    sock.connect(t.server.services["dns"]).await.unwrap();
    let mut buf = [0u8; 64];
    let start = std::time::Instant::now();
    let mut got = 0;
    while start.elapsed() < Duration::from_secs(6) {
        sock.send(b"tick").await.unwrap();
        if let Ok(Ok(_)) =
            tokio::time::timeout(Duration::from_millis(100), sock.recv(&mut buf)).await
        {
            got += 1;
        }
    }
    assert!(
        got >= 3,
        "sparse replies must keep arriving while requests flow, got {got}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stopping_the_client_closes_its_data_channels() {
    let echo = udp_echo_server().await;
    let t = tunnel(vec![("dns", service(echo, Protocol::Udp))]).await;
    let sock = udp_bind().await;
    sock.connect(t.server.services["dns"]).await.unwrap();
    let mut buf = [0u8; 64];
    let mut ok = false;
    for _ in 0..50 {
        sock.send(b"x").await.unwrap();
        if tokio::time::timeout(Duration::from_millis(100), sock.recv(&mut buf))
            .await
            .is_ok()
        {
            ok = true;
            break;
        }
    }
    assert!(ok);
    t.client.stop.send(true).unwrap();
    t.client.task.await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    while tokio::time::timeout(Duration::from_millis(50), sock.recv(&mut buf))
        .await
        .is_ok()
    {}
    sock.send(b"after stop").await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(700), sock.recv(&mut buf))
            .await
            .is_err(),
        "a stopped client must not keep relaying"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removing_a_client_closes_its_open_connections() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let settings = server_settings(vec![secret.clone()]);
    let mut t = tunnel_with(
        settings.clone(),
        vec![("web", service(echo, Protocol::Tcp))],
        |c| c,
        secret,
    )
    .await;
    let conn = open_echo(t.server.services["web"]).await;
    let mut next = settings;
    next.clients.clear();
    t.server.reload(next).await;
    wait_state(&mut t.client.state, State::Rejected).await;
    assert_cut(conn).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removing_the_secret_in_use_closes_its_open_connections() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let settings = server_settings(vec![secret.clone()]);
    let t = tunnel_with(
        settings.clone(),
        vec![("web", service(echo, Protocol::Tcp))],
        |c| c,
        secret,
    )
    .await;
    let conn = open_echo(t.server.services["web"]).await;
    let mut next = settings;
    next.services
        .insert("web".into(), service(echo, Protocol::Tcp));
    next.clients.get_mut("home").unwrap().secrets = vec![Secret::generate()];
    t.server.reload(next).await;
    let shared = t.server.shared.clone();
    wait_for(|| shared.session_names().is_empty()).await;
    assert_cut(conn).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn adding_a_secret_keeps_open_connections() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let settings = server_settings(vec![secret.clone()]);
    let t = tunnel_with(
        settings.clone(),
        vec![("web", service(echo, Protocol::Tcp))],
        |c| c,
        secret.clone(),
    )
    .await;
    let mut conn = open_echo(t.server.services["web"]).await;
    let mut next = settings;
    next.services
        .insert("web".into(), service(echo, Protocol::Tcp));
    next.clients.get_mut("home").unwrap().secrets = vec![secret, Secret::generate()];
    t.server.reload_applied(next).await;
    conn.write_all(b"pong").await.unwrap();
    let mut buf = [0u8; 4];
    conn.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"pong");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reload_can_swap_the_ports_of_two_services() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let settings = server_settings(vec![secret.clone()]);
    let t = tunnel_with(
        settings.clone(),
        vec![
            ("a", service(echo, Protocol::Tcp)),
            ("b", service(echo, Protocol::Tcp)),
        ],
        |c| c,
        secret,
    )
    .await;
    let (pa, pb) = (t.server.services["a"], t.server.services["b"]);
    let mut next = settings;
    next.listen = Some(t.server.addr);
    let mut a = service(echo, Protocol::Tcp);
    a.bind = Some(pb);
    let mut b = service(echo, Protocol::Tcp);
    b.bind = Some(pa);
    next.services.insert("a".into(), a);
    next.services.insert("b".into(), b);
    t.server.reload(next).await;
    for addr in [pa, pb] {
        let mut ok = false;
        for _ in 0..50 {
            let probe = async {
                let mut c = TcpStream::connect(addr).await.ok()?;
                c.write_all(b"swap").await.ok()?;
                let mut buf = [0u8; 4];
                c.read_exact(&mut buf).await.ok()?;
                Some(buf)
            };
            if tokio::time::timeout(Duration::from_millis(500), probe)
                .await
                .ok()
                .flatten()
                == Some(*b"swap")
            {
                ok = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(ok, "{addr} serves again after the swap");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reload_onto_the_kept_control_port_is_rejected() {
    if ws_mode() {
        return;
    }
    let echo = echo_server().await;
    let secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    settings.listen = Some(free_addr());
    let t = tunnel_with(
        settings.clone(),
        vec![("a", service(echo, Protocol::Tcp))],
        |c| c,
        secret,
    )
    .await;
    let old = t.server.services["a"];
    let mut next = settings;
    next.listen = Some("127.0.0.1:1".parse().unwrap());
    let mut a = service(echo, Protocol::Tcp);
    a.bind = Some(t.server.addr);
    next.services.insert("a".into(), a);
    let report = t.server.reload_applied(next).await;
    assert!(report.rejected.is_some(), "{report:?}");
    assert_eq!(roundtrip(old, b"kept").await, b"kept");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_client_connection_limit_holds_under_a_burst() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    settings.clients.get_mut("home").unwrap().max_connections = Some(2);
    let t = tunnel_with(
        settings,
        vec![("web", service(echo, Protocol::Tcp))],
        |c| c,
        secret,
    )
    .await;
    let addr = t.server.services["web"];
    let mut tries = tokio::task::JoinSet::new();
    for _ in 0..10 {
        tries.spawn(async move {
            let mut c = TcpStream::connect(addr).await.ok()?;
            c.write_all(b"x").await.ok()?;
            let mut b = [0u8; 1];
            tokio::time::timeout(Duration::from_secs(3), c.read_exact(&mut b))
                .await
                .ok()?
                .ok()?;
            Some(c)
        });
    }
    let open: Vec<TcpStream> = tries.join_all().await.into_iter().flatten().collect();
    assert!(
        open.len() <= 2,
        "{} connections got through a limit of 2",
        open.len()
    );
    assert!(!open.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_burst_on_a_public_service_wider_than_the_handshake_limit_still_gets_through() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    settings.max_handshakes = 2;
    let t = tunnel_with(
        settings,
        vec![("web", service(echo, Protocol::Tcp))],
        |c| c,
        secret,
    )
    .await;
    let addr = t.server.services["web"];
    let mut tasks = tokio::task::JoinSet::new();
    for i in 0..20u64 {
        tasks.spawn(async move {
            let data = pattern(2_000, i);
            assert_eq!(roundtrip(addr, &data).await, data);
        });
    }
    while let Some(r) = tasks.join_next().await {
        r.unwrap();
    }
    assert!(
        t.server.shared.status().handshakes_refused > 0,
        "the burst should have hit the limit"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_burst_wider_than_max_pending_to_one_client_waits_and_is_served() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    settings.max_pending = 8;
    settings.pool_min_idle = 1;
    settings.pool_max_idle = 1;
    let t = tunnel_with(
        settings,
        vec![("web", service(echo, Protocol::Tcp))],
        |c| c,
        secret,
    )
    .await;
    let addr = t.server.services["web"];
    let mut tasks = tokio::task::JoinSet::new();
    for i in 0..32u64 {
        tasks.spawn(async move {
            let data = pattern(2_000, i);
            assert_eq!(roundtrip(addr, &data).await, data);
        });
    }
    while let Some(r) = tasks.join_next().await {
        r.unwrap();
    }
    assert_eq!(t.server.shared.waiting_visitors(), 0);
}
