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

const BIG: usize = 64 * 1024 * 1024;

async fn big_app() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            tokio::spawn(async move {
                if read_head(&mut s).await.is_none() {
                    return;
                }
                let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {BIG}\r\n\r\n");
                if s.write_all(head.as_bytes()).await.is_err() {
                    return;
                }
                let chunk = vec![b'z'; 64 * 1024];
                for _ in 0..BIG / chunk.len() {
                    if s.write_all(&chunk).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    addr
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reload_that_no_longer_allows_a_visitor_cuts_a_fast_apps_answer_it_reads_slowly() {
    let secret = Secret::generate();
    let mut svc = site(&["home"], big_app().await);
    svc.sources.allow = vec!["127.0.0.0/8".parse().unwrap()];
    let s = plain_server(&secret, &[("app", svc, &["app.example.com"])]);
    let web = s.web.as_ref().unwrap().http.unwrap();
    let t = tunnel_with(s.clone(), Vec::new(), |c| c, secret).await;
    let sock = tokio::net::TcpSocket::new_v4().unwrap();
    sock.bind("127.0.0.2:0".parse().unwrap()).unwrap();
    let mut v = sock.connect(web).await.unwrap();
    v.write_all(b"GET /big HTTP/1.1\r\nHost: app.example.com\r\n\r\n")
        .await
        .unwrap();
    let mut buf = vec![0u8; 64 * 1024];
    let mut got = v.read(&mut buf).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut next: ServerSettings = s.clone();
    next.services.get_mut("app").unwrap().sources.allow = vec!["127.0.0.3/32".parse().unwrap()];
    t.server.reload_applied(next).await;
    let read = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match v.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(n) => got += n,
            }
        }
    })
    .await;
    assert!(read.is_ok(), "the answer ended within 20 s");
    assert!(
        got < BIG,
        "the answer is cut, as it was before the pool: {got} of {BIG} bytes arrived"
    );
}

async fn silent_app() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            tokio::spawn(async move {
                let _ = read_head(&mut s).await;
                let mut rest = [0u8; 1];
                let _ = s.read(&mut rest).await;
            });
        }
    });
    addr
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reload_that_no_longer_allows_a_visitor_cuts_a_request_still_waiting_for_its_answer() {
    let secret = Secret::generate();
    let mut svc = site(&["home"], silent_app().await);
    svc.sources.allow = vec!["127.0.0.0/8".parse().unwrap()];
    let s = plain_server(&secret, &[("app", svc, &["app.example.com"])]);
    let web = s.web.as_ref().unwrap().http.unwrap();
    let t = tunnel_with(s.clone(), Vec::new(), |c| c, secret).await;
    let sock = tokio::net::TcpSocket::new_v4().unwrap();
    sock.bind("127.0.0.2:0".parse().unwrap()).unwrap();
    let mut v = sock.connect(web).await.unwrap();
    v.write_all(b"GET /poll HTTP/1.1\r\nHost: app.example.com\r\n\r\n")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut next: ServerSettings = s.clone();
    next.services.get_mut("app").unwrap().sources.allow = vec!["127.0.0.3/32".parse().unwrap()];
    t.server.reload_applied(next).await;
    let mut buf = [0u8; 512];
    let ended = tokio::time::timeout(Duration::from_secs(10), v.read(&mut buf)).await;
    assert!(
        ended.is_ok(),
        "the waiting request is cut at the reload, as it was before the pool"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ntlm_handshake_stays_on_one_app_connection_with_balance() {
    let (a, log_a) = recording_app(Behave::Ntlm).await;
    let (b, log_b) = recording_app(Behave::Ntlm).await;
    let secret = Secret::generate();
    let s = plain_server(
        &secret,
        &[(
            "app",
            two_entries(Mode::Balance, a, b),
            &["app.example.com"],
        )],
    );
    let web = s.web.as_ref().unwrap().http.unwrap();
    let _t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    for v in 0..4 {
        let mut keep = Keep::open(web, localhost()).await;
        let first = keep
            .send("GET", "/", &format!("x-id: {v}-1\r\n"))
            .await
            .unwrap();
        assert_eq!(status(&first), 401);
        for step in 2..=3 {
            let extra = format!("x-id: {v}-{step}\r\nAuthorization: NTLM step{step}\r\n");
            let got = keep.send("GET", "/", &extra).await.unwrap();
            assert_eq!(status(&got), 200, "visitor {v} step {step}: {got}");
        }
    }
    for v in 0..4 {
        let mut placed = Vec::new();
        for (side, log) in [("a", &log_a), ("b", &log_b)] {
            for r in log.asked() {
                if r.id
                    .as_deref()
                    .is_some_and(|id| id.starts_with(&format!("{v}-")))
                {
                    placed.push((side, r.conn));
                }
            }
        }
        assert_eq!(placed.len(), 3, "{placed:?}");
        assert!(
            placed.iter().all(|p| *p == placed[0]),
            "visitor {v}'s handshake moved between app connections: {placed:?}"
        );
    }
}
