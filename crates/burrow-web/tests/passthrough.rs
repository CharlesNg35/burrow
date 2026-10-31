mod common;

use std::time::Duration;

use common::*;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

#[tokio::test]
async fn each_name_reaches_its_own_backend_with_its_own_certificate() {
    let (nas, mut nas_names) = backend("nas", &["nas.example.com"]).await;
    let (git, mut git_names) = backend("git", &["git.example.com"]).await;
    let (up, _) = Fake::new(4);
    up.serve("nas", nas);
    up.serve("git", git);
    let proxy = start(
        |s| {
            s.sites = vec![
                site("nas", &["nas.example.com"]),
                site("git", &["git.example.com"]),
            ];
        },
        up.clone(),
    )
    .await;
    let addr = proxy.https_addr().unwrap();
    let mut a = tls(addr, "NAS.example.com").await.unwrap();
    assert_eq!(ask(&mut a, b"hi").await.unwrap(), b"nas:hi");
    assert_eq!(
        nas_names.recv().await.unwrap().as_deref(),
        Some("nas.example.com")
    );
    let mut b = tls(addr, "git.example.com").await.unwrap();
    assert_eq!(ask(&mut b, b"yo").await.unwrap(), b"git:yo");
    assert_eq!(
        git_names.recv().await.unwrap().as_deref(),
        Some("git.example.com")
    );
    let seen = up.visitors.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].0, "nas");
    assert_eq!(seen[0].1.host, "nas.example.com");
    assert_eq!(seen[0].1.local, addr);
    assert_eq!(seen[0].1.addr.ip(), addr.ip());
    proxy.stop().await;
}

#[tokio::test]
async fn unknown_names_and_clients_without_a_name_are_closed_without_a_certificate() {
    let (nas, _names) = backend("nas", &["nas.example.com"]).await;
    let (up, _) = Fake::new(4);
    up.serve("nas", nas);
    let proxy = start(
        |s| s.sites = vec![site("nas", &["nas.example.com"])],
        up.clone(),
    )
    .await;
    let addr = proxy.https_addr().unwrap();
    assert!(closed_without_tls(addr, "other.example.com").await);
    assert!(closed_without_tls(addr, "10.9.9.9").await);
    let mut plain = TcpStream::connect(addr).await.unwrap();
    let got = ask(
        &mut plain,
        b"GET / HTTP/1.1\r\nHost: nas.example.com\r\n\r\n",
    )
    .await;
    assert!(matches!(got.as_deref(), Ok([]) | Err(_)), "{got:?}");
    assert!(up.visitors.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_ip_site_answers_clients_that_send_no_name() {
    let (kiosk, mut names) = backend("kiosk", &["127.0.0.1"]).await;
    let (up, _) = Fake::new(4);
    up.serve("kiosk", kiosk);
    let proxy = start(
        |s| s.sites = vec![site("kiosk", &["127.0.0.1"])],
        up.clone(),
    )
    .await;
    let mut a = tls(proxy.https_addr().unwrap(), "127.0.0.1").await.unwrap();
    assert_eq!(ask(&mut a, b"x").await.unwrap(), b"kiosk:x");
    assert_eq!(names.recv().await.unwrap(), None);
}

#[tokio::test]
async fn a_wildcard_site_takes_one_label_and_an_exact_site_wins() {
    let (apps, _a) = backend("apps", &["x.apps.example.com"]).await;
    let (one, _o) = backend("one", &["one.apps.example.com"]).await;
    let (up, _) = Fake::new(4);
    up.serve("apps", apps);
    up.serve("one", one);
    let proxy = start(
        |s| {
            s.sites = vec![
                site("apps", &["*.apps.example.com"]),
                site("one", &["one.apps.example.com"]),
            ]
        },
        up.clone(),
    )
    .await;
    let addr = proxy.https_addr().unwrap();
    let mut x = tls(addr, "x.apps.example.com").await.unwrap();
    assert_eq!(ask(&mut x, b"1").await.unwrap(), b"apps:1");
    let mut o = tls(addr, "one.apps.example.com").await.unwrap();
    assert_eq!(ask(&mut o, b"2").await.unwrap(), b"one:2");
    assert!(closed_without_tls(addr, "a.b.apps.example.com").await);
}

#[tokio::test]
async fn a_refused_open_closes_the_visitor() {
    let (up, _) = Fake::new(4);
    *up.refuse.lock().unwrap() = Some(burrow_web::Refused::Full);
    let proxy = start(
        |s| s.sites = vec![site("nas", &["nas.example.com"])],
        up.clone(),
    )
    .await;
    assert!(closed_without_tls(proxy.https_addr().unwrap(), "nas.example.com").await);
}

#[tokio::test]
async fn a_backend_reset_reaches_the_visitor_as_a_reset() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 64];
        let _ = s.read(&mut buf).await;
        socket2::SockRef::from(&s)
            .set_linger(Some(Duration::ZERO))
            .unwrap();
    });
    let (up, _) = Fake::new(4);
    up.serve("nas", backend);
    let proxy = start(
        |s| s.sites = vec![site("nas", &["nas.example.com"])],
        up.clone(),
    )
    .await;
    let err = tls(proxy.https_addr().unwrap(), "nas.example.com")
        .await
        .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::ConnectionReset, "{err}");
}

#[tokio::test]
async fn hosts_added_and_removed_take_effect_for_new_connections() {
    let (nas, _n) = backend("nas", &["nas.example.com"]).await;
    let (git, _g) = backend("git", &["git.example.com"]).await;
    let (up, _) = Fake::new(4);
    up.serve("nas", nas);
    up.serve("git", git);
    let proxy = start(
        |s| s.sites = vec![site("nas", &["nas.example.com"])],
        up.clone(),
    )
    .await;
    let addr = proxy.https_addr().unwrap();
    let mut kept = tls(addr, "nas.example.com").await.unwrap();
    assert_eq!(ask(&mut kept, b"a").await.unwrap(), b"nas:a");
    assert!(closed_without_tls(addr, "git.example.com").await);
    let mut next = burrow_web::ProxySettings::new(addr);
    next.sites = vec![site("git", &["git.example.com"])];
    proxy.apply(&next).await;
    let mut g = tls(addr, "git.example.com").await.unwrap();
    assert_eq!(ask(&mut g, b"b").await.unwrap(), b"git:b");
    assert!(closed_without_tls(addr, "nas.example.com").await);
    assert_eq!(
        ask(&mut kept, b"c").await.unwrap(),
        b"nas:c",
        "an open connection is closed by the server's reload check, not by the table"
    );
}

async fn silent(addr: std::net::SocketAddr, n: usize) -> Vec<TcpStream> {
    let mut held = Vec::new();
    for _ in 0..n {
        held.push(TcpStream::connect(addr).await.unwrap());
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    held
}

async fn closes_within(s: &mut TcpStream, limit: Duration) -> bool {
    let mut buf = [0u8; 1];
    let ended = tokio::time::timeout(limit, s.read(&mut buf)).await;
    matches!(ended, Ok(Ok(0)) | Ok(Err(_)))
}

#[tokio::test]
async fn silent_connections_over_the_cap_make_others_wait_their_turn_and_a_stop_ends_the_wait() {
    let (nas, _n) = backend("nas", &["nas.example.com"]).await;
    let (up, _) = Fake::new(4);
    up.serve("nas", nas);
    let proxy = start(
        |s| {
            s.sites = vec![site("nas", &["nas.example.com"])];
            s.max_pending = 4;
            s.timeouts.handshake = Duration::from_secs(3);
        },
        up.clone(),
    )
    .await;
    let addr = proxy.https_addr().unwrap();
    let mut held = silent(addr, 4).await;
    let waiting = tokio::spawn(async move { tls(addr, "nas.example.com").await });
    wait_until("the 5th is accepted", || proxy.admitted() == 5).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !waiting.is_finished(),
        "the 5th waits instead of being closed"
    );
    drop(held.pop());
    let mut ok = waiting.await.unwrap().expect("served once a slot is free");
    assert_eq!(ask(&mut ok, b"z").await.unwrap(), b"nas:z");
    assert_eq!(proxy.refused(), 0);
    held.extend(silent(addr, 1).await);
    let mut late = silent(addr, 1).await.pop().unwrap();
    assert!(
        closes_within(&mut late, Duration::from_secs(5)).await,
        "a silent waiter is closed by its own deadline"
    );
    assert!(proxy.refused() >= 1, "it waited and then said nothing");
    let mut cut = silent(addr, 5).await;
    proxy.stop().await;
    for s in &mut cut {
        assert!(
            closes_within(s, Duration::from_secs(1)).await,
            "a stop ends every wait at once"
        );
    }
}
