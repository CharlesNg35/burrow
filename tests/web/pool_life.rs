use std::net::SocketAddr;
use std::time::Duration;

use burrow::config::settings::{Entry, ForwardAuth, ServerSettings};
use burrow::wire::secret::Secret;
use burrow_config::Mode;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::common::*;
use crate::kit::*;
use crate::pool_kit::*;

fn localhost() -> std::net::IpAddr {
    "127.0.0.1".parse().unwrap()
}

fn two_entries(mode: Mode, a: SocketAddr, b: SocketAddr) -> burrow::config::settings::Service {
    let mut svc = site(&["home", "self"], a);
    svc.mode = mode;
    svc.entries = vec![
        Entry::new("home", Some(a.to_string()), 1, &svc.target),
        Entry::new("self", Some(b.to_string()), 1, &svc.target),
    ];
    svc
}

async fn two_apps(
    mode: Mode,
) -> (
    Tunnel,
    SocketAddr,
    (std::sync::Arc<Log>, std::sync::Arc<Log>),
    Secret,
) {
    let (a, log_a) = recording_app(Behave::Answer).await;
    let (b, log_b) = recording_app(Behave::Answer).await;
    let secret = Secret::generate();
    let s = plain_server(
        &secret,
        &[("app", two_entries(mode, a, b), &["app.example.com"])],
    );
    let web = s.web.as_ref().unwrap().http.unwrap();
    let t = tunnel_with(s, Vec::new(), |c| c, secret.clone()).await;
    (t, web, (log_a, log_b), secret)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn balance_picks_per_request_for_one_kept_visitor() {
    let (_t, web, (a, b), _) = two_apps(Mode::Balance).await;
    let mut v = Keep::open(web, localhost()).await;
    for _ in 0..100 {
        assert_eq!(status(&v.get("/").await), 200);
    }
    let (na, nb) = (a.asked().len(), b.asked().len());
    assert_eq!(na + nb, 100);
    assert!((40..=60).contains(&na), "{na} and {nb}");
    assert!(
        a.conns() <= 2 && b.conns() <= 2,
        "each side reuses its app connection"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failover_moves_to_the_second_client_when_the_first_leaves_and_back_when_it_returns() {
    let (t, web, (a, b), secret) = two_apps(Mode::Failover).await;
    let mut v = Keep::open(web, localhost()).await;
    for _ in 0..3 {
        assert_eq!(status(&v.get("/").await), 200);
    }
    assert_eq!((a.asked().len(), b.asked().len()), (3, 0));
    let _ = t.client.stop.send(true);
    let _ = t.client.task.await;
    let shared = t.server.shared.clone();
    wait_for(|| !shared.session_names().iter().any(|n| n == "home")).await;
    for _ in 0..3 {
        assert_eq!(status(&v.get("/").await), 200);
    }
    assert_eq!(b.asked().len(), 3, "self took over");
    let mut again = start_client(client_settings(t.server.addr, secret)).await;
    wait_state(&mut again.state, burrow::client::State::Connected).await;
    wait_for(|| shared.idle_channels("home") >= 1).await;
    for _ in 0..3 {
        assert_eq!(status(&v.get("/").await), 200);
    }
    assert_eq!(a.asked().len(), 6, "home is first again");
}

async fn events_app() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            tokio::spawn(async move {
                if read_head(&mut s).await.is_none() {
                    return;
                }
                let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n";
                if s.write_all(head.as_bytes()).await.is_err() {
                    return;
                }
                loop {
                    if s.write_all(b"6\r\ndata:\n\r\n").await.is_err() {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            });
        }
    });
    addr
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reload_that_no_longer_allows_a_visitor_cuts_its_running_answer() {
    let secret = Secret::generate();
    let mut svc = site(&["home"], events_app().await);
    svc.sources.allow = vec!["127.0.0.0/8".parse().unwrap()];
    let s = plain_server(&secret, &[("app", svc, &["app.example.com"])]);
    let web = s.web.as_ref().unwrap().http.unwrap();
    let t = tunnel_with(s.clone(), Vec::new(), |c| c, secret).await;
    let sock = tokio::net::TcpSocket::new_v4().unwrap();
    sock.bind("127.0.0.2:0".parse().unwrap()).unwrap();
    let mut v = sock.connect(web).await.unwrap();
    v.write_all(b"GET /events HTTP/1.1\r\nHost: app.example.com\r\n\r\n")
        .await
        .unwrap();
    let mut buf = [0u8; 512];
    let n = v.read(&mut buf).await.unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200"));
    let mut next: ServerSettings = s.clone();
    next.services.get_mut("app").unwrap().sources.allow = vec!["127.0.0.3/32".parse().unwrap()];
    t.server.reload_applied(next).await;
    let cut = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match v.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
        }
    })
    .await;
    assert!(cut.is_ok(), "the answer is cut, as it was before the pool");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forward_auth_checks_reuse_one_app_connection_of_the_login_service() {
    let (app, _) = recording_app(Behave::Answer).await;
    let (login, checks) = recording_app(Behave::Answer).await;
    let secret = Secret::generate();
    let mut s = plain_server(
        &secret,
        &[("app", site(&["home"], app), &["app.example.com"])],
    );
    s.services.insert("login".into(), site(&["home"], login));
    let options = &mut s
        .web
        .as_mut()
        .unwrap()
        .sites
        .get_mut("app")
        .unwrap()
        .options;
    options.forward_auth = Some(ForwardAuth {
        service: "login".into(),
        path: "/check".into(),
        copy_headers: Vec::new(),
        strip_headers: Vec::new(),
    });
    let web = s.web.as_ref().unwrap().http.unwrap();
    let _t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    for _ in 0..10 {
        let got = plain_get(web, "app.example.com", "/").await;
        assert!(got.starts_with("HTTP/1.1 200"), "{got}");
    }
    assert_eq!(checks.asked().len(), 10);
    assert!(checks.conns() <= 2, "{} login connections", checks.conns());
}
