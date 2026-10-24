use std::time::{Duration, Instant};

use burrow::wire::secret::Secret;
use burrow_config::ProxyVersion;
use tokio::io::AsyncReadExt;

use crate::common::*;
use crate::kit::*;

async fn up(s: burrow::config::settings::ServerSettings, secret: Secret) -> Tunnel {
    tunnel_with(s, Vec::new(), |c| c, secret).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_name_reaches_its_backend_through_a_remote_client_or_self() {
    let (nas_cert, git_cert) = (cert(&["nas.example.com"]), cert(&["git.example.com"]));
    let (nas, mut nas_seen) = backend("nas", &nas_cert, false).await;
    let (git, mut git_seen) = backend("git", &git_cert, false).await;
    let secret = Secret::generate();
    let s = web_server(
        &secret,
        &[
            ("nas", site(&["home"], nas), &["nas.example.com"]),
            ("git", site(&["self"], git), &["git.example.com"]),
        ],
    );
    let t = up(s, secret).await;
    let web = t.server.web_addr.expect("web listener");
    let mut a = visit(web, "nas.example.com", &nas_cert).await.unwrap();
    assert_eq!(ask(&mut a, b"hello").await.unwrap(), b"nas:hello");
    assert_eq!(
        nas_seen.recv().await.unwrap().sni.as_deref(),
        Some("nas.example.com")
    );
    let mut b = visit(web, "git.example.com", &git_cert).await.unwrap();
    assert_eq!(ask(&mut b, b"hi").await.unwrap(), b"git:hi");
    assert_eq!(
        git_seen.recv().await.unwrap().sni.as_deref(),
        Some("git.example.com")
    );
    assert!(
        visit(web, "git.example.com", &nas_cert).await.is_err(),
        "each app shows its own certificate"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unknown_names_and_hellos_without_a_name_are_closed() {
    let nas_cert = cert(&["nas.example.com"]);
    let (nas, _seen) = backend("nas", &nas_cert, false).await;
    let secret = Secret::generate();
    let s = web_server(
        &secret,
        &[("nas", site(&["home"], nas), &["nas.example.com"])],
    );
    let t = up(s, secret).await;
    let web = t.server.web_addr.unwrap();
    assert!(closed(web, "other.example.com").await);
    assert!(closed(web, "127.0.0.1").await, "no SNI and no IP site");
    let status = t.server.shared.status();
    let nas = status.services.iter().find(|s| s.name == "nas").unwrap();
    assert_eq!(nas.total, 0, "nothing reached the service");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refusing_client_fails_over_with_the_hello_replayed() {
    let nas_cert = cert(&["nas.example.com"]);
    let (nas, mut seen) = backend("nas", &nas_cert, false).await;
    let secret = Secret::generate();
    let office = Secret::generate();
    let mut s = web_server(
        &secret,
        &[("nas", site(&["office", "home"], nas), &["nas.example.com"])],
    );
    s.clients.insert(
        "office".into(),
        burrow::config::settings::ClientAuth {
            secrets: vec![office.clone()],
            max_connections: None,
        },
    );
    let t = up(s, secret).await;
    let mut o = client_settings(t.server.addr, office);
    o.name = "office".into();
    o.allow = burrow::policy::allowlist::AllowList::Rules(vec!["10.0.0.0/8".parse().unwrap()]);
    let mut oc = start_client(o).await;
    wait_state(&mut oc.state, burrow::client::State::Connected).await;
    let shared = t.server.shared.clone();
    wait_for(|| shared.idle_channels("office") >= 1).await;
    for i in 0..3 {
        let mut a = visit(t.server.web_addr.unwrap(), "nas.example.com", &nas_cert)
            .await
            .unwrap_or_else(|e| panic!("visit {i}: {e}"));
        assert_eq!(ask(&mut a, b"x").await.unwrap(), b"nas:x");
        assert!(seen.recv().await.is_some());
    }
    let status = t.server.shared.status();
    let nas = status.services.iter().find(|s| s.name == "nas").unwrap();
    assert!(
        nas.set_aside
            .iter()
            .any(|x| x.client == "office" && x.refused),
        "office refused the target first: {:?}",
        nas.set_aside
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn allow_from_and_max_connections_apply_to_websites() {
    let nas_cert = cert(&["nas.example.com"]);
    let (nas, _seen) = backend("nas", &nas_cert, false).await;
    let secret = Secret::generate();
    let mut limited = site(&["home"], nas);
    limited.max_connections = Some(1);
    let mut denied = site(&["home"], nas);
    denied.sources.allow = vec!["10.0.0.0/8".parse().unwrap()];
    let s = web_server(
        &secret,
        &[
            ("nas", limited, &["nas.example.com"]),
            ("lan", denied, &["lan.example.com"]),
        ],
    );
    let t = up(s, secret).await;
    let web = t.server.web_addr.unwrap();
    let mut first = visit(web, "nas.example.com", &nas_cert).await.unwrap();
    assert_eq!(ask(&mut first, b"1").await.unwrap(), b"nas:1");
    assert!(closed(web, "nas.example.com").await, "max_connections: 1");
    drop(first);
    let shared = t.server.shared.clone();
    wait_for(|| shared.active_connections() == 0).await;
    let mut again = visit(web, "nas.example.com", &nas_cert).await.unwrap();
    assert_eq!(ask(&mut again, b"2").await.unwrap(), b"nas:2");
    assert!(closed(web, "lan.example.com").await, "allow_from");
    let status = t.server.shared.status();
    let lan = status.services.iter().find(|s| s.name == "lan").unwrap();
    assert_eq!((lan.total, lan.refused), (1, 1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_app_gets_a_proxy_v2_header_with_the_visitor_and_the_web_port() {
    let nas_cert = cert(&["nas.example.com"]);
    let (nas, mut seen) = backend("nas", &nas_cert, true).await;
    let secret = Secret::generate();
    let mut svc = site(&["home"], nas);
    svc.proxy_protocol = Some(ProxyVersion::V2);
    let s = web_server(&secret, &[("nas", svc, &["nas.example.com"])]);
    let t = up(s, secret).await;
    let web = t.server.web_addr.unwrap();
    let mut a = visit(web, "nas.example.com", &nas_cert).await.unwrap();
    let local = a.get_ref().0.local_addr().unwrap();
    assert_eq!(ask(&mut a, b"p").await.unwrap(), b"nas:p");
    let header = seen.recv().await.unwrap().proxy.unwrap();
    assert_eq!(header[12], 0x21);
    assert_eq!(header[13], 0x11);
    let port = |at: usize| u16::from_be_bytes([header[16 + at], header[17 + at]]);
    assert_eq!(&header[16..20], &[127, 0, 0, 1]);
    assert_eq!(port(8), local.port(), "the visitor's port");
    assert_eq!(port(10), web.port(), "the web proxy's port");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_websites_bandwidth_is_limited_like_any_service() {
    let nas_cert = cert(&["nas.example.com"]);
    let (nas, _seen) = backend("nas", &nas_cert, false).await;
    let secret = Secret::generate();
    let mut svc = site(&["home"], nas);
    svc.bandwidth = Some(128 * 1024);
    let s = web_server(&secret, &[("nas", svc, &["nas.example.com"])]);
    let t = up(s, secret).await;
    let a = visit_with_small_send_buffer(t.server.web_addr.unwrap(), "nas.example.com", &nas_cert)
        .await
        .unwrap();
    let data = pattern(384 * 1024, 9);
    let started = Instant::now();
    let (mut r, mut w) = tokio::io::split(a);
    let want = data.len();
    let reader = tokio::spawn(async move {
        let (mut back, mut buf) = (0, vec![0u8; 64 * 1024]);
        while back < want {
            let n = tokio::time::timeout(Duration::from_secs(20), r.read(&mut buf))
                .await
                .unwrap()
                .unwrap();
            assert!(n > 0);
            back += n;
        }
    });
    tokio::io::AsyncWriteExt::write_all(&mut w, &data)
        .await
        .unwrap();
    tokio::io::AsyncWriteExt::flush(&mut w).await.unwrap();
    reader.await.unwrap();
    let took = started.elapsed();
    assert!(
        took >= Duration::from_secs(2),
        "384 KiB at 128 KiB/s took {took:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_that_resets_reaches_the_visitor_as_a_reset() {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let app = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            let mut buf = [0u8; 64];
            let _ = s.read(&mut buf).await;
            socket2::SockRef::from(&s)
                .set_linger(Some(Duration::ZERO))
                .unwrap();
        }
    });
    let secret = Secret::generate();
    let s = web_server(
        &secret,
        &[("nas", site(&["home"], app), &["nas.example.com"])],
    );
    let t = up(s, secret).await;
    let err = visit(t.server.web_addr.unwrap(), "nas.example.com", &cert(&["x"]))
        .await
        .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::ConnectionReset, "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shutdown_waits_for_open_websites_and_counts_them() {
    let nas_cert = cert(&["nas.example.com"]);
    let (nas, _seen) = backend("nas", &nas_cert, false).await;
    let secret = Secret::generate();
    let s = web_server(
        &secret,
        &[("nas", site(&["home"], nas), &["nas.example.com"])],
    );
    let t = up(s, secret).await;
    let mut a = visit(t.server.web_addr.unwrap(), "nas.example.com", &nas_cert)
        .await
        .unwrap();
    assert_eq!(ask(&mut a, b"x").await.unwrap(), b"nas:x");
    assert_eq!(t.server.shared.active_connections(), 1);
    t.server.stop.send(true).unwrap();
    let mut task = t.server.task;
    let early = tokio::time::timeout(Duration::from_millis(700), &mut task).await;
    assert!(
        early.is_err(),
        "the drain waits for the open website connection"
    );
    drop(a);
    let done = tokio::time::timeout(Duration::from_secs(5), task).await;
    assert!(done.is_ok(), "the server stops once it has closed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_self_service_cannot_dial_the_web_port_back_into_burrow() {
    let nas_cert = cert(&["nas.example.com"]);
    let (nas, _seen) = backend("nas", &nas_cert, false).await;
    let secret = Secret::generate();
    let port = free_port();
    let mut s = web_server(
        &secret,
        &[("nas", site(&["home"], nas), &["nas.example.com"])],
    );
    s.web.as_mut().unwrap().https = format!("127.0.0.1:{port}").parse().unwrap();
    let mut back = service(
        format!("127.0.0.1:{port}").parse().unwrap(),
        burrow_config::Protocol::Tcp,
    );
    back.clients = vec!["self".into()];
    s.services.insert("back".into(), back);
    let t = up(s, secret).await;
    let direct = visit(t.server.web_addr.unwrap(), "nas.example.com", &nas_cert).await;
    assert!(direct.is_ok(), "the site works when dialled directly");
    let looped = visit(t.server.services["back"], "nas.example.com", &nas_cert).await;
    assert!(
        looped.is_err(),
        "the self dialer never reaches burrow's own web port"
    );
}
