use std::time::Duration;

use burrow::config::settings::WebSite;
use burrow::wire::secret::Secret;

use crate::common::*;
use crate::kit::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hosts_added_and_removed_on_save_take_effect_and_close_what_they_removed() {
    let (nas_cert, git_cert) = (cert(&["nas.example.com"]), cert(&["git.example.com"]));
    let (nas, _a) = backend("nas", &nas_cert, false).await;
    let (git, _b) = backend("git", &git_cert, false).await;
    let secret = Secret::generate();
    let s = web_server(
        &secret,
        &[("nas", site(&["home"], nas), &["nas.example.com"])],
    );
    let t = tunnel_with(s.clone(), Vec::new(), |c| c, secret.clone()).await;
    let web = t.server.web_addr.unwrap();
    let mut open = visit(web, "nas.example.com", &nas_cert).await.unwrap();
    assert_eq!(ask(&mut open, b"a").await.unwrap(), b"nas:a");
    assert!(closed(web, "git.example.com").await);

    let mut next = s.clone();
    next.services.insert("git".into(), site(&["home"], git));
    next.web.as_mut().unwrap().sites.insert(
        "git".into(),
        WebSite::passthrough(vec!["git.example.com".into()]),
    );
    t.server.reload(next.clone()).await;
    let shared = t.server.shared.clone();
    wait_for(|| shared.settings_snapshot().web == next.web).await;
    let mut g = visit(web, "git.example.com", &git_cert).await.unwrap();
    assert_eq!(ask(&mut g, b"b").await.unwrap(), b"git:b");
    assert_eq!(
        ask(&mut open, b"c").await.unwrap(),
        b"nas:c",
        "a kept host keeps its connections"
    );

    let mut fewer = next.clone();
    fewer.web.as_mut().unwrap().sites.remove("nas");
    t.server.reload(fewer.clone()).await;
    wait_for(|| shared.settings_snapshot().web == fewer.web).await;
    assert!(closed(web, "nas.example.com").await);
    let cut = tokio::time::timeout(Duration::from_secs(5), ask(&mut open, b"d")).await;
    assert!(
        !matches!(&cut, Ok(Ok(v)) if v == b"nas:d"),
        "an open connection to a removed host is closed: {cut:?}"
    );
    assert_eq!(t.server.web_addr, Some(web), "the listener stayed");
    let mut still = visit(web, "git.example.com", &git_cert).await.unwrap();
    assert_eq!(ask(&mut still, b"e").await.unwrap(), b"git:e");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn turning_the_web_proxy_off_and_on_closes_and_opens_its_port() {
    let nas_cert = cert(&["nas.example.com"]);
    let (nas, _a) = backend("nas", &nas_cert, false).await;
    let secret = Secret::generate();
    let port = free_port();
    let mut s = web_server(
        &secret,
        &[("nas", site(&["home"], nas), &["nas.example.com"])],
    );
    s.web.as_mut().unwrap().https = Some(format!("127.0.0.1:{port}").parse().unwrap());
    let t = tunnel_with(s.clone(), Vec::new(), |c| c, secret.clone()).await;
    let web = t.server.web_addr.unwrap();
    assert_eq!(web.port(), port);
    let mut off = s.clone();
    off.web = None;
    t.server.reload(off).await;
    let shared = t.server.shared.clone();
    wait_for(|| shared.settings_snapshot().web.is_none()).await;
    wait_for(|| std::net::TcpStream::connect(web).is_err()).await;
    t.server.reload(s.clone()).await;
    wait_for(|| shared.settings_snapshot().web.is_some()).await;
    wait_for(|| std::net::TcpStream::connect(web).is_ok()).await;
    let mut a = visit(web, "nas.example.com", &nas_cert).await.unwrap();
    assert_eq!(ask(&mut a, b"on").await.unwrap(), b"nas:on");
}

fn separate_ws(s: &mut burrow::config::settings::ServerSettings, port: u16, c: &Cert) {
    share_tunnel(s, "localhost", c);
    let ws = s.websocket.as_mut().unwrap();
    ws.listen = format!("127.0.0.1:{port}").parse().unwrap();
    ws.host = None;
    s.web.as_mut().unwrap().tunnel = None;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn moving_the_tunnel_onto_the_web_port_and_back_never_binds_it_twice() {
    logs();
    let tunnel_cert = cert(&["localhost"]);
    let nas_cert = cert(&["nas.example.com"]);
    let (nas, _a) = backend("nas", &nas_cert, false).await;
    let secret = Secret::generate();
    let (ws_port, web_port) = (free_port(), free_port());
    let mut apart = web_server(
        &secret,
        &[("nas", site(&["home"], nas), &["nas.example.com"])],
    );
    apart.web.as_mut().unwrap().https = Some(format!("127.0.0.1:{web_port}").parse().unwrap());
    separate_ws(&mut apart, ws_port, &tunnel_cert);
    let srv = start_server(apart.clone()).await;
    assert_eq!(srv.ws_addr.map(|a| a.port()), Some(ws_port));
    let ws_url = format!("wss://localhost:{ws_port}/burrow");
    let mut first = start_client(crate::tunnel::wss_client(
        &ws_url,
        secret.clone(),
        &tunnel_cert,
    ))
    .await;
    wait_state(&mut first.state, burrow::client::State::Connected).await;

    let mut shared_cfg = apart.clone();
    share_tunnel(&mut shared_cfg, "localhost", &tunnel_cert);
    srv.reload(shared_cfg.clone()).await;
    let shared = srv.shared.clone();
    wait_for(|| shared.settings_snapshot().websocket == shared_cfg.websocket).await;
    wait_for(|| std::net::TcpStream::connect(("127.0.0.1", ws_port)).is_err()).await;
    tokio::time::timeout(
        Duration::from_secs(5),
        first
            .state
            .wait_for(|s| *s != burrow::client::State::Connected),
    )
    .await
    .expect("the client of the moved listener is told to reconnect")
    .unwrap();
    let web_url = format!("wss://localhost:{web_port}/burrow");
    let mut second = start_client(crate::tunnel::wss_client(
        &web_url,
        secret.clone(),
        &tunnel_cert,
    ))
    .await;
    wait_state(&mut second.state, burrow::client::State::Connected).await;
    let mut a = visit(
        format!("127.0.0.1:{web_port}").parse().unwrap(),
        "nas.example.com",
        &nas_cert,
    )
    .await
    .unwrap();
    assert_eq!(ask(&mut a, b"x").await.unwrap(), b"nas:x");

    srv.reload(apart.clone()).await;
    wait_for(|| shared.settings_snapshot().websocket == apart.websocket).await;
    wait_for(|| std::net::TcpStream::connect(("127.0.0.1", ws_port)).is_ok()).await;
    let web: std::net::SocketAddr = format!("127.0.0.1:{web_port}").parse().unwrap();
    assert!(
        closed(web, "localhost").await,
        "the tunnel's name left the web port"
    );
    wait_state(&mut first.state, burrow::client::State::Connected).await;
    let mut b = visit(web, "nas.example.com", &nas_cert).await.unwrap();
    assert_eq!(ask(&mut b, b"y").await.unwrap(), b"nas:y");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_service_bind_and_the_web_port_can_swap_places() {
    let nas_cert = cert(&["nas.example.com"]);
    let (nas, _a) = backend("nas", &nas_cert, false).await;
    let echo = echo_server().await;
    let secret = Secret::generate();
    let (p1, p2) = (free_port(), free_port());
    let at = |p: u16| -> std::net::SocketAddr { format!("127.0.0.1:{p}").parse().unwrap() };
    let mut s = web_server(
        &secret,
        &[("nas", site(&["home"], nas), &["nas.example.com"])],
    );
    s.web.as_mut().unwrap().https = Some(at(p2));
    let mut db = service(echo, burrow_config::Protocol::Tcp);
    db.bind = Some(at(p1));
    s.services.insert("db".into(), db.clone());
    let t = tunnel_with(s.clone(), Vec::new(), |c| c, secret.clone()).await;
    assert_eq!(
        try_roundtrip(at(p1), b"db").await.as_deref(),
        Some(&b"db"[..])
    );
    let mut swapped = s.clone();
    swapped.web.as_mut().unwrap().https = Some(at(p1));
    db.bind = Some(at(p2));
    swapped.services.insert("db".into(), db);
    t.server.reload(swapped).await;
    let ok = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let web = visit(at(p1), "nas.example.com", &nas_cert).await.is_ok();
            let svc = try_roundtrip(at(p2), b"db").await.as_deref() == Some(&b"db"[..]);
            if web && svc {
                return;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await;
    assert!(ok.is_ok(), "both moved to the other's old port");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_tunnel_certificate_is_used_for_new_connections() {
    let (old_cert, new_cert) = (cert(&["localhost"]), cert(&["localhost"]));
    let secret = Secret::generate();
    let mut s = web_server(&secret, &[]);
    s.web.as_mut().unwrap().https = Some(format!("127.0.0.1:{}", free_port()).parse().unwrap());
    share_tunnel(&mut s, "localhost", &old_cert);
    let srv = start_server(s.clone()).await;
    let web = srv.web_addr.unwrap();
    assert!(visit(web, "localhost", &old_cert).await.is_ok());
    assert!(visit(web, "localhost", &new_cert).await.is_err());
    let mut next = s.clone();
    share_tunnel(&mut next, "localhost", &new_cert);
    srv.reload(next.clone()).await;
    let shared = srv.shared.clone();
    wait_for(|| shared.settings_snapshot().websocket == next.websocket).await;
    let mut ok = false;
    for _ in 0..50 {
        if visit(web, "localhost", &new_cert).await.is_ok() {
            ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(ok, "the new certificate is served");
    assert_eq!(srv.web_addr, Some(web));
}
