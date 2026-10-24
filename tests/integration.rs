mod common;

use std::time::Duration;

use burrow::client::State;
use burrow::policy::allowlist::{AllowList, Rule};
use burrow::wire::proxy::ProxyConfig;
use burrow::wire::secret::Secret;
use burrow_config::{Cipher, Protocol};
use common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tcp_echo_through_tunnel() {
    let echo = echo_server().await;
    let t = tunnel(vec![("web", service(echo, Protocol::Tcp))]).await;
    let addr = t.server.services["web"];
    assert_eq!(roundtrip(addr, b"hello burrow").await, b"hello burrow");
    assert_eq!(roundtrip(addr, b"").await, b"");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn large_transfer_keeps_checksum() {
    let echo = echo_server().await;
    let t = tunnel(vec![("web", service(echo, Protocol::Tcp))]).await;
    let data = pattern(64 << 20, 7);
    let got = roundtrip(t.server.services["web"], &data).await;
    assert_eq!(got.len(), data.len());
    assert!(got == data, "payload corrupted");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn many_parallel_connections() {
    let echo = echo_server().await;
    let t = tunnel(vec![("web", service(echo, Protocol::Tcp))]).await;
    let addr = t.server.services["web"];
    let many = if cfg!(any(target_os = "macos", target_os = "freebsd")) {
        100
    } else {
        300
    };
    let tasks: Vec<_> = (0..many)
        .map(|i| {
            tokio::spawn(async move {
                let data = pattern(1000 + i * 37, i as u64);
                assert_eq!(roundtrip(addr, &data).await, data);
            })
        })
        .collect();
    for t in tasks {
        t.await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn half_close_is_propagated_both_ways() {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = l.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut s, _) = l.accept().await.unwrap();
        let mut got = Vec::new();
        s.read_to_end(&mut got).await.unwrap();
        s.write_all(format!("got {} bytes", got.len()).as_bytes())
            .await
            .unwrap();
        s.shutdown().await.unwrap();
    });
    let t = tunnel(vec![("web", service(target, Protocol::Tcp))]).await;
    let mut s = TcpStream::connect(t.server.services["web"]).await.unwrap();
    s.write_all(&[1u8; 12345]).await.unwrap();
    s.shutdown().await.unwrap();
    let mut reply = String::new();
    s.read_to_string(&mut reply).await.unwrap();
    assert_eq!(reply, "got 12345 bytes");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_reconnects_after_server_restart() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    settings
        .services
        .insert("web".into(), service(echo, Protocol::Tcp));
    let srv = start_server(settings.clone()).await;
    let addr = srv.addr;
    let svc_addr = srv.services["web"];
    let mut cli = start_client(client_settings(addr, secret)).await;
    wait_state(&mut cli.state, State::Connected).await;

    srv.stop.send(true).unwrap();
    srv.task.await.unwrap();
    wait_state(&mut cli.state, State::Connecting).await;

    settings.listen = Some(addr);
    settings.services.get_mut("web").unwrap().bind = Some(svc_addr);
    let srv2 = start_server(settings).await;
    wait_state(&mut cli.state, State::Connected).await;
    let shared = srv2.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 1).await;
    assert_eq!(roundtrip(svc_addr, b"again").await, b"again");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hot_reload_add_change_remove_without_dropping() {
    let echo1 = echo_server().await;
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let echo2 = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = l.accept().await.unwrap();
            tokio::spawn(async move {
                let mut b = Vec::new();
                s.read_to_end(&mut b).await.unwrap();
                s.write_all(b"two").await.unwrap();
            });
        }
    });
    let secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    settings
        .services
        .insert("web".into(), service(echo1, Protocol::Tcp));
    let t = tunnel_with(settings.clone(), vec![], |c| c, secret).await;
    let web = t.server.services["web"];

    let mut long = TcpStream::connect(web).await.unwrap();
    long.write_all(b"ping").await.unwrap();
    let mut buf = [0u8; 4];
    long.read_exact(&mut buf).await.unwrap();

    let free = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let new_addr = free.local_addr().unwrap();
    drop(free);
    let mut next = settings.clone();
    next.services.get_mut("web").unwrap().bind = Some(web);
    let mut added = service(echo1, Protocol::Tcp);
    added.bind = Some(new_addr);
    next.services.insert("api".into(), added);
    t.server.reload(next.clone()).await;
    wait_for(|| std::net::TcpStream::connect(new_addr).is_ok()).await;
    assert_eq!(roundtrip(new_addr, b"api").await, b"api");

    next.services.get_mut("web").unwrap().target = echo2.to_string();
    t.server.reload(next.clone()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(roundtrip(web, b"x").await, b"two");

    next.services.remove("api");
    t.server.reload(next.clone()).await;
    wait_for(|| std::net::TcpStream::connect(new_addr).is_err()).await;

    long.write_all(b"pong").await.unwrap();
    long.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"pong");
    assert_eq!(*t.client.state.borrow(), State::Connected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalid_reload_is_rejected_and_old_config_kept() {
    use burrow::config::settings::ServerSettings;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.yaml");
    let secret = Secret::generate();
    let good = format!(
        "listen: 127.0.0.1:1\nclients:\n  home:\n    secret: {}\nservices:\n  web:\n    client: home\n    bind: 127.0.0.1:2\n    target: 127.0.0.1:3\n",
        secret.encode()
    );
    std::fs::write(&path, &good).unwrap();
    let initial = match burrow::config::load_file(&path, None, &burrow::config::env::ProcessEnv)
        .unwrap()
        .loaded
    {
        Some(burrow::config::Loaded::Server(s, _)) => *s,
        _ => panic!("valid config"),
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel::<ServerSettings>(4);
    tokio::spawn(burrow::reload::follow(
        path.clone(),
        initial,
        burrow::config::env::Env::default(),
        burrow::reload::accept(),
        tx,
    ));
    tokio::time::sleep(Duration::from_millis(300)).await;

    std::fs::write(&path, good.replace("client: home", "client: hom")).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(1200), rx.recv())
            .await
            .is_err()
    );

    std::fs::write(&path, good.replace("127.0.0.1:3", "127.0.0.1:4")).unwrap();
    let next = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next.services["web"].target, "127.0.0.1:4");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn allow_list_denies_targets_outside_it() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let t = tunnel_with(
        server_settings(vec![secret.clone()]),
        vec![("web", service(echo, Protocol::Tcp))],
        |mut c| {
            c.allow = AllowList::Rules(vec!["10.0.0.0/8".parse::<Rule>().unwrap()]);
            c
        },
        secret,
    )
    .await;
    let got = try_roundtrip(t.server.services["web"], b"secret data").await;
    assert!(
        got.is_none_or(|g| g.is_empty()),
        "denied target must not receive or return data"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn allow_list_port_rule() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let rule = format!("127.0.0.1/32:{}", echo.port());
    let t = tunnel_with(
        server_settings(vec![secret.clone()]),
        vec![("web", service(echo, Protocol::Tcp))],
        |mut c| {
            c.allow = AllowList::Rules(vec![rule.parse::<Rule>().unwrap()]);
            c
        },
        secret,
    )
    .await;
    assert_eq!(roundtrip(t.server.services["web"], b"ok").await, b"ok");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn udp_forwarding_round_trips() {
    let echo = udp_echo_server().await;
    let t = tunnel(vec![("dns", service(echo, Protocol::Udp))]).await;
    let target = t.server.services["dns"];
    let mut sockets = Vec::new();
    for _ in 0..3 {
        let s = udp_bind().await;
        s.connect(target).await.unwrap();
        sockets.push(s);
    }
    let mut buf = vec![0u8; 65536];
    for round in 0..20u32 {
        for (i, s) in sockets.iter().enumerate() {
            let msg = format!("peer {i} round {round}");
            let mut got = false;
            'retry: for _ in 0..50 {
                s.send(msg.as_bytes()).await.unwrap();
                while let Ok(Ok(n)) =
                    tokio::time::timeout(Duration::from_millis(200), s.recv(&mut buf)).await
                {
                    if &buf[..n] == msg.as_bytes() {
                        got = true;
                        break 'retry;
                    }
                }
            }
            assert!(got, "no echo for {msg}");
        }
    }
    let big = pattern(60000, 1);
    sockets[0].send(&big).await.unwrap();
    loop {
        let n = tokio::time::timeout(Duration::from_secs(2), sockets[0].recv(&mut buf))
            .await
            .unwrap()
            .unwrap();
        if n == big.len() {
            assert_eq!(&buf[..n], &big[..]);
            break;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn graceful_shutdown_lets_active_connections_finish() {
    let echo = echo_server().await;
    let t = tunnel(vec![("web", service(echo, Protocol::Tcp))]).await;
    let mut s = TcpStream::connect(t.server.services["web"]).await.unwrap();
    s.write_all(b"before").await.unwrap();
    let mut buf = [0u8; 6];
    s.read_exact(&mut buf).await.unwrap();

    t.server.stop.send(true).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !t.server.task.is_finished(),
        "server waits for the active connection"
    );
    s.write_all(b"during").await.unwrap();
    s.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"during");
    s.shutdown().await.unwrap();
    let mut rest = Vec::new();
    s.read_to_end(&mut rest).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), t.server.task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn insecure_plaintext_mode_forwards() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let mut s = server_settings(vec![secret.clone()]);
    s.insecure_plaintext = true;
    let t = tunnel_with(
        s,
        vec![("web", service(echo, Protocol::Tcp))],
        |mut c| {
            c.insecure_plaintext = true;
            c
        },
        secret,
    )
    .await;
    let data = pattern(5 << 20, 3);
    assert_eq!(roundtrip(t.server.services["web"], &data).await, data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plaintext_mismatch_is_refused() {
    let secret = Secret::generate();
    let srv = start_server(server_settings(vec![secret.clone()])).await;
    let mut c = client_settings(srv.addr, secret);
    c.insecure_plaintext = true;
    let cli = start_client(c).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_ne!(*cli.state.borrow(), State::Connected);
    assert!(srv.shared.session_names().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn aes_gcm_client_works() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let t = tunnel_with(
        server_settings(vec![secret.clone()]),
        vec![("web", service(echo, Protocol::Tcp))],
        |mut c| {
            c.cipher = Cipher::AesGcm;
            c
        },
        secret,
    )
    .await;
    let data = pattern(4 << 20, 9);
    assert_eq!(roundtrip(t.server.services["web"], &data).await, data);
}

async fn http_connect_proxy() -> std::net::SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = l.accept().await.unwrap();
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut b = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if s.read(&mut b).await.unwrap() == 0 {
                        return;
                    }
                    head.push(b[0]);
                }
                let line = String::from_utf8_lossy(&head).to_string();
                let target = line.split_whitespace().nth(1).unwrap().to_string();
                let mut up = TcpStream::connect(target).await.unwrap();
                s.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                    .await
                    .unwrap();
                let _ = tokio::io::copy_bidirectional(&mut s, &mut up).await;
            });
        }
    });
    addr
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_connects_through_http_proxy() {
    let echo = echo_server().await;
    let proxy = http_connect_proxy().await;
    let secret = Secret::generate();
    let t = tunnel_with(
        server_settings(vec![secret.clone()]),
        vec![("web", service(echo, Protocol::Tcp))],
        |mut c| {
            c.proxy = Some(ProxyConfig::parse(&format!("http://{proxy}")).unwrap());
            c
        },
        secret,
    )
    .await;
    assert_eq!(
        roundtrip(t.server.services["web"], b"via proxy").await,
        b"via proxy"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn per_service_connection_limit() {
    let echo = echo_server().await;
    let mut svc = service(echo, Protocol::Tcp);
    svc.max_connections = Some(2);
    let t = tunnel(vec![("web", svc)]).await;
    let addr = t.server.services["web"];
    let mut held = Vec::new();
    for _ in 0..2 {
        let mut s = TcpStream::connect(addr).await.unwrap();
        s.write_all(b"x").await.unwrap();
        let mut b = [0u8; 1];
        s.read_exact(&mut b).await.unwrap();
        held.push(s);
    }
    let got = try_roundtrip(addr, b"third").await;
    assert!(
        got.is_none_or(|g| g.is_empty()),
        "third connection must be refused"
    );
    drop(held);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(roundtrip(addr, b"later").await, b"later");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removed_client_is_disconnected() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    settings
        .services
        .insert("web".into(), service(echo, Protocol::Tcp));
    let mut t = tunnel_with(settings.clone(), vec![], |c| c, secret).await;
    let mut next = settings;
    next.clients.clear();
    next.services.clear();
    t.server.reload(next).await;
    wait_state(&mut t.client.state, State::Rejected).await;
}

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
    t.server.reload(next).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
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
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    settings.listen = Some(free.local_addr().unwrap());
    drop(free);
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
    t.server.reload(next).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
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

async fn black_hole_proxy(
    to: std::net::SocketAddr,
) -> (
    std::net::SocketAddr,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::{AtomicBool, Ordering};
    let hole = std::sync::Arc::new(AtomicBool::new(false));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let h = hole.clone();
    tokio::spawn(async move {
        while let Ok((inner, _)) = listener.accept().await {
            let Ok(outer) = TcpStream::connect(to).await else {
                continue;
            };
            let (ir, iw) = inner.into_split();
            let (or, ow) = outer.into_split();
            for (mut from, mut into, hole) in [
                (
                    Box::new(ir) as Box<dyn tokio::io::AsyncRead + Unpin + Send>,
                    Box::new(ow) as Box<dyn tokio::io::AsyncWrite + Unpin + Send>,
                    h.clone(),
                ),
                (Box::new(or) as _, Box::new(iw) as _, h.clone()),
            ] {
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 16 * 1024];
                    while let Ok(n) = from.read(&mut buf).await {
                        if n == 0 {
                            break;
                        }
                        if !hole.load(Ordering::Relaxed) && into.write_all(&buf[..n]).await.is_err()
                        {
                            break;
                        }
                    }
                    if !hole.load(Ordering::Relaxed) {
                        let _ = into.shutdown().await;
                    }
                    std::future::pending::<()>().await;
                });
            }
        }
    });
    (addr, hole)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_connection_on_a_dead_data_channel_ends_instead_of_hanging() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let srv = start_server({
        let mut s = server_settings(vec![secret.clone()]);
        s.services
            .insert("web".into(), service(echo, Protocol::Tcp));
        s
    })
    .await;
    let (proxy, hole) = black_hole_proxy(srv.addr).await;
    let mut cli = start_client(client_settings(proxy, secret)).await;
    wait_state(&mut cli.state, State::Connected).await;
    let shared = srv.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 1).await;
    hole.store(true, std::sync::atomic::Ordering::Relaxed);
    let mut user = TcpStream::connect(srv.services["web"]).await.unwrap();
    user.write_all(b"anyone there?").await.unwrap();
    let mut buf = [0u8; 64];
    let ended = tokio::time::timeout(Duration::from_secs(10), user.read(&mut buf)).await;
    assert!(
        matches!(ended, Ok(Ok(0)) | Ok(Err(_))),
        "the connection should be closed, not left hanging: {ended:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reload_with_ports_taken_by_other_programs_does_not_stall() {
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
    let mut held = Vec::new();
    let mut next = t.server.shared.settings_snapshot();
    for name in ["a1", "a2", "a3"] {
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut busy = service(echo, Protocol::Tcp);
        busy.bind = Some(taken.local_addr().unwrap());
        next.services.insert(name.into(), busy);
        held.push(taken);
    }
    let (_hold, free) = refusing();
    drop(_hold);
    let mut last = service(echo, Protocol::Tcp);
    last.bind = Some(free);
    next.services.insert("zz".into(), last);
    let began = std::time::Instant::now();
    t.server.reload(next).await;
    let mut reachable = false;
    while began.elapsed() < Duration::from_secs(10) {
        if TcpStream::connect(free).await.is_ok() {
            reachable = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let took = began.elapsed();
    assert!(reachable, "the free service was never bound");
    assert!(
        took < Duration::from_secs(1),
        "the reload stalled for {took:?}"
    );
    let data = pattern(1_000, 3);
    assert_eq!(roundtrip(t.server.services["web"], &data).await, data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_service_whose_port_was_busy_at_reload_listens_once_it_is_free() {
    let echo = echo_server().await;
    let t = tunnel(vec![("web", service(echo, Protocol::Tcp))]).await;
    let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = busy.local_addr().unwrap();
    let mut next = server_settings(vec![t.secret.clone()]);
    next.listen = Some(t.server.addr);
    let mut web = service(echo, Protocol::Tcp);
    web.bind = Some(t.server.services["web"]);
    next.services.insert("web".into(), web);
    let mut late = service(echo, Protocol::Tcp);
    late.bind = Some(port);
    next.services.insert("late".into(), late);
    t.server.reload(next).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(roundtrip(t.server.services["web"], b"kept").await, b"kept");
    drop(busy);
    let mut up = false;
    for _ in 0..30 {
        if try_roundtrip(port, b"late").await.as_deref() == Some(&b"late"[..]) {
            up = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(up, "the service never listened after its port was freed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn making_a_service_private_closes_its_public_connections() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    settings
        .services
        .insert("web".into(), service(echo, Protocol::Tcp));
    settings
        .visitors
        .insert("laptop".into(), visitor_auth(Secret::generate(), &[]));
    let t = tunnel_with(settings.clone(), vec![], |c| c, secret).await;
    let open = open_echo(t.server.services["web"]).await;
    let mut next = settings.clone();
    let web = next.services.get_mut("web").unwrap();
    web.bind = None;
    web.visitors = vec!["laptop".into()];
    t.server.reload(next).await;
    assert_cut(open).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_symlinked_config_is_followed_through_its_links() {
    use burrow::config::settings::ServerSettings;
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let secret = Secret::generate();
    let config = |target: &str| {
        format!(
            "listen: 127.0.0.1:1\nclients:\n  home:\n    secret: {}\nservices:\n  web:\n    client: home\n    bind: 127.0.0.1:2\n    target: {target}\n",
            secret.encode()
        )
    };
    let version = |n: u32, target: &str| {
        let v = dir.path().join(format!("..2026_{n}"));
        std::fs::create_dir(&v).unwrap();
        std::fs::write(v.join("server.yaml"), config(target)).unwrap();
        v
    };
    let first = version(1, "127.0.0.1:3");
    symlink(&first, dir.path().join("..data")).unwrap();
    let path = dir.path().join("server.yaml");
    symlink("..data/server.yaml", &path).unwrap();
    let initial = match burrow::config::load_file(&path, None, &burrow::config::env::ProcessEnv)
        .unwrap()
        .loaded
    {
        Some(burrow::config::Loaded::Server(s, _)) => *s,
        _ => panic!("valid config"),
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel::<ServerSettings>(4);
    tokio::spawn(burrow::reload::follow(
        path.clone(),
        initial,
        burrow::config::env::Env::default(),
        burrow::reload::accept(),
        tx,
    ));
    tokio::time::sleep(Duration::from_millis(300)).await;
    let second = version(2, "127.0.0.1:4");
    symlink(&second, dir.path().join("..data_tmp")).unwrap();
    std::fs::rename(dir.path().join("..data_tmp"), dir.path().join("..data")).unwrap();
    let next = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("a ConfigMap-style swap is seen")
        .unwrap();
    assert_eq!(next.services["web"].target, "127.0.0.1:4");
    let file = second.join("server.yaml");
    std::fs::write(&file, config("127.0.0.1:5")).unwrap();
    let next = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("an edit of the file a link points to is seen")
        .unwrap();
    assert_eq!(next.services["web"].target, "127.0.0.1:5");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reloaded_udp_session_timeout_applies_to_running_relays() {
    let echo = udp_echo_server().await;
    let secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    settings.udp_session_timeout = Duration::from_secs(30);
    let mut dns = service(echo, Protocol::Udp);
    dns.bind = Some(
        std::net::UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap(),
    );
    settings.services.insert("dns".into(), dns);
    let t = tunnel_with(settings.clone(), vec![], |c| c, secret).await;
    let target = t.server.services["dns"];
    let s = udp_bind().await;
    s.connect(target).await.unwrap();
    let mut buf = [0u8; 16];
    for _ in 0..50 {
        s.send(b"hi").await.unwrap();
        if tokio::time::timeout(Duration::from_millis(200), s.recv(&mut buf))
            .await
            .is_ok()
        {
            break;
        }
    }
    let shared = t.server.shared.clone();
    assert_eq!(shared.udp_peers(), 1);
    let mut next = settings.clone();
    next.udp_session_timeout = Duration::from_secs(1);
    t.server.reload(next).await;
    let mut gone = false;
    for _ in 0..80 {
        if shared.udp_peers() == 0 {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(gone, "the shorter timeout did not expire the idle peer");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_save_between_start_and_the_file_watch_is_not_missed() {
    use burrow::config::settings::ServerSettings;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.yaml");
    let secret = Secret::generate();
    std::fs::write(
        &path,
        format!(
            "listen: 127.0.0.1:1\nclients:\n  home:\n    secret: {}\nservices:\n  web:\n    client: home\n    bind: 127.0.0.1:2\n    target: 127.0.0.1:4\n",
            secret.encode()
        ),
    )
    .unwrap();
    let mut started = match burrow::config::load_file(&path, None, &burrow::config::env::ProcessEnv)
        .unwrap()
        .loaded
    {
        Some(burrow::config::Loaded::Server(s, _)) => *s,
        _ => panic!("valid config"),
    };
    started.services.get_mut("web").unwrap().target = "127.0.0.1:3".into();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<ServerSettings>(4);
    tokio::spawn(burrow::reload::follow(
        path.clone(),
        started,
        burrow::config::env::Env::default(),
        burrow::reload::accept(),
        tx,
    ));
    let next = tokio::time::timeout(Duration::from_secs(3), rx.recv())
        .await
        .expect("the file on disk is applied without waiting for another change")
        .unwrap();
    assert_eq!(next.services["web"].target, "127.0.0.1:4");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_udp_target_named_localhost_reaches_an_ipv4_only_listener() {
    let echo = udp_echo_server().await;
    let mut svc = service(echo, Protocol::Udp);
    svc.target = format!("localhost:{}", echo.port());
    let secret = Secret::generate();
    let t = tunnel_with(
        server_settings(vec![secret.clone()]),
        vec![("dns", svc)],
        |mut c| {
            c.allow = burrow::policy::allowlist::AllowList::Rules(vec![
                "[::1/128]".parse().unwrap(),
                "127.0.0.0/8".parse().unwrap(),
            ]);
            c
        },
        secret,
    )
    .await;
    let s = udp_bind().await;
    s.connect(t.server.services["dns"]).await.unwrap();
    let mut buf = [0u8; 64];
    let mut got = false;
    for _ in 0..30 {
        s.send(b"ping").await.unwrap();
        if let Ok(Ok(n)) = tokio::time::timeout(Duration::from_millis(200), s.recv(&mut buf)).await
            && &buf[..n] == b"ping"
        {
            got = true;
            break;
        }
    }
    assert!(got, "no echo through a localhost UDP target");
}

async fn plaintext_tunnel(plaintext: bool, target: std::net::SocketAddr) -> Tunnel {
    let secret = Secret::generate();
    let mut s = server_settings(vec![secret.clone()]);
    s.insecure_plaintext = plaintext;
    tunnel_with(
        s,
        vec![("web", service(target, Protocol::Tcp))],
        move |mut c| {
            c.insecure_plaintext = plaintext;
            c
        },
        secret,
    )
    .await
}

fn reset_on_drop(s: &TcpStream) {
    socket2::SockRef::from(s)
        .set_linger(Some(Duration::ZERO))
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_target_reset_mid_response_reaches_the_user_as_a_reset() {
    for plaintext in [false, true] {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = l.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            s.write_all(b"partial").await.unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
            reset_on_drop(&s);
        });
        let t = plaintext_tunnel(plaintext, target).await;
        let mut user = TcpStream::connect(t.server.services["web"]).await.unwrap();
        let mut got = Vec::new();
        let end = tokio::time::timeout(Duration::from_secs(5), user.read_to_end(&mut got))
            .await
            .unwrap();
        assert_eq!(got, b"partial", "plaintext {plaintext}");
        assert!(
            matches!(&end, Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset),
            "plaintext {plaintext}: a cut-off response must not end like a complete one: {end:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_user_reset_mid_upload_reaches_the_target_as_a_reset() {
    for plaintext in [false, true] {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = l.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut got = Vec::new();
            let end = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut got)).await;
            let _ = tx.send((got, end.map(|r| r.map_err(|e| e.kind()))));
        });
        let t = plaintext_tunnel(plaintext, target).await;
        let user = TcpStream::connect(t.server.services["web"]).await.unwrap();
        let mut user = user;
        user.write_all(b"partial").await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        reset_on_drop(&user);
        drop(user);
        let (got, end) = rx.await.unwrap();
        assert_eq!(got, b"partial", "plaintext {plaintext}");
        assert_eq!(
            end,
            Ok(Err(std::io::ErrorKind::ConnectionReset)),
            "plaintext {plaintext}: a cut-off upload must not end like a complete one"
        );
    }
}
