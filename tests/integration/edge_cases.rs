use std::time::Duration;

use burrow::client::State;
use burrow::wire::secret::Secret;
use burrow_config::Protocol;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::common::*;

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
    let busy = std::net::TcpListener::bind(free_addr()).unwrap();
    let port = busy.local_addr().unwrap();
    let mut next = server_settings(vec![t.secret.clone()]);
    next.listen = Some(t.server.addr);
    let mut web = service(echo, Protocol::Tcp);
    web.bind = Some(t.server.services["web"]);
    next.services.insert("web".into(), web);
    let mut late = service(echo, Protocol::Tcp);
    late.bind = Some(port);
    next.services.insert("late".into(), late);
    let report = t.server.reload_applied(next).await;
    assert_eq!(report.unbound, ["\"late\""], "{report:?}");
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
    tokio::spawn(burrow::app::reload::follow(
        path.clone(),
        initial,
        burrow::config::env::Env::default(),
        burrow::app::reload::accept(),
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
    dns.bind = Some(free_addr());
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
    tokio::spawn(burrow::app::reload::follow(
        path.clone(),
        started,
        burrow::config::env::Env::default(),
        burrow::app::reload::accept(),
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
