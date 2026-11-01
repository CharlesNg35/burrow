use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use burrow::config::settings::ServerSettings;
use burrow::wire::secret::Secret;

use crate::common::*;
use crate::kit::*;
use crate::pool_kit::*;

fn plain(app: SocketAddr, tune: impl FnOnce(&mut ServerSettings)) -> (ServerSettings, Secret) {
    let secret = Secret::generate();
    let mut s = plain_server(
        &secret,
        &[("app", site(&["home"], app), &["app.example.com"])],
    );
    tune(&mut s);
    (s, secret)
}

fn http(s: &ServerSettings) -> SocketAddr {
    s.web.as_ref().unwrap().http.unwrap()
}

fn localhost() -> std::net::IpAddr {
    "127.0.0.1".parse().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn new_visitor_connections_share_the_app_connections_of_earlier_ones() {
    let (app, log) = recording_app(Behave::Answer).await;
    let (s, secret) = plain(app, |_| {});
    let web = http(&s);
    let _t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    for _ in 0..20 {
        let got = plain_get(web, "app.example.com", "/").await;
        assert!(got.starts_with("HTTP/1.1 200"), "{got}");
    }
    assert!(
        log.conns() <= 2,
        "20 visitors, {} app connections",
        log.conns()
    );
}

fn seen(log: &Log, id: &str) -> usize {
    log.asked()
        .iter()
        .filter(|a| a.id.as_deref() == Some(id))
        .count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_connection_the_app_closed_is_not_used_and_get_and_post_are_each_sent_once() {
    let (app, log) = recording_app(Behave::CloseAfterAnswer).await;
    let (s, secret) = plain(app, |_| {});
    let web = http(&s);
    let _t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    let mut v = Keep::open(web, localhost()).await;
    assert_eq!(status(&v.get("/").await), 200);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut v = Keep::open(web, localhost()).await;
    let got = v.send("GET", "/", "x-id: get\r\n").await.unwrap();
    assert_eq!(status(&got), 200, "{got}");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let got = v.send("POST", "/", "x-id: post\r\n").await.unwrap();
    assert_eq!(status(&got), 200, "{got}");
    assert_eq!(seen(&log, "get"), 1);
    assert_eq!(seen(&log, "post"), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_get_lost_on_a_reused_connection_is_sent_again_once_and_a_post_never() {
    let (app, log) = recording_app(Behave::CloseOnReuse).await;
    let (s, secret) = plain(app, |_| {});
    let web = http(&s);
    let _t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    let mut v = Keep::open(web, localhost()).await;
    assert_eq!(
        status(&v.send("GET", "/", "x-id: a\r\n").await.unwrap()),
        200
    );
    let got = v.send("GET", "/", "x-id: b\r\n").await.unwrap();
    assert_eq!(status(&got), 200, "sent again on a new connection: {got}");
    assert_eq!(
        seen(&log, "b"),
        2,
        "the app saw it twice, as Go and nginx allow"
    );
    let got = v.send("POST", "/", "x-id: c\r\n").await.unwrap();
    assert_eq!(status(&got), 502, "{got}");
    assert_eq!(seen(&log, "c"), 1, "a POST is never sent twice");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_connection_that_fails_is_never_sent_again() {
    let (app, log) = recording_app(Behave::CloseAlways).await;
    let (s, secret) = plain(app, |_| {});
    let web = http(&s);
    let _t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    let mut v = Keep::open(web, localhost()).await;
    let got = v.send("GET", "/", "x-id: fresh\r\n").await.unwrap();
    assert_eq!(status(&got), 502, "{got}");
    assert_eq!(seen(&log, "fresh"), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_request_reaches_the_app_more_often_than_the_rules_allow() {
    let (app, log) = recording_app(Behave::CloseSometimes).await;
    let (s, secret) = plain(app, |_| {});
    let web = http(&s);
    let _t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    let mut methods = HashMap::new();
    for round in 0..4 {
        let mut v = Keep::open(web, localhost()).await;
        for i in 0..15 {
            let method = if i % 3 == 0 { "POST" } else { "GET" };
            let id = format!("{round}-{i}");
            methods.insert(id.clone(), method);
            if v.send(method, "/", &format!("x-id: {id}\r\n"))
                .await
                .is_none()
            {
                v = Keep::open(web, localhost()).await;
            }
        }
    }
    for (id, method) in methods {
        let n = seen(&log, &id);
        let most = if method == "POST" { 1 } else { 2 };
        assert!(n <= most, "{method} {id} reached the app {n} times");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn after_the_client_reconnects_the_next_requests_get_answers() {
    let (app, _log) = recording_app(Behave::Answer).await;
    let (s, secret) = plain(app, |_| {});
    let web = http(&s);
    let t = tunnel_with(s, Vec::new(), |c| c, secret.clone()).await;
    for _ in 0..3 {
        assert!(
            plain_get(web, "app.example.com", "/")
                .await
                .starts_with("HTTP/1.1 200")
        );
    }
    let _ = t.client.stop.send(true);
    let _ = t.client.task.await;
    let shared = t.server.shared.clone();
    wait_for(|| shared.session_names().is_empty()).await;
    let mut again = start_client(client_settings(t.server.addr, secret)).await;
    wait_state(&mut again.state, burrow::client::State::Connected).await;
    wait_for(|| shared.idle_channels("home") >= 1).await;
    for _ in 0..10 {
        let got = plain_get(web, "app.example.com", "/").await;
        assert!(got.starts_with("HTTP/1.1 200"), "{got}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ntlm_connection_stays_with_the_visitor_connection_that_started_it() {
    let (app, log) = recording_app(Behave::Ntlm).await;
    let (s, secret) = plain(app, |_| {});
    let web = http(&s);
    let _t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    let mut a = Keep::open(web, localhost()).await;
    assert_eq!(status(&a.get("/").await), 401);
    let mut b = Keep::open(web, localhost()).await;
    assert_eq!(status(&b.get("/").await), 401);
    let got = a
        .send("GET", "/", "Authorization: NTLM step2\r\n")
        .await
        .unwrap();
    assert_eq!(status(&got), 200, "{got}");
    let asked = log.asked();
    let first = asked[0].conn;
    assert_ne!(
        asked[1].conn, first,
        "another visitor gets its own app connection"
    );
    assert_eq!(
        asked[2].conn, first,
        "the handshake goes on over the same one"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_server_stops_promptly_with_idle_app_connections() {
    let (app, _log) = recording_app(Behave::Answer).await;
    let (s, secret) = plain(app, |_| {});
    let web = http(&s);
    let t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    for _ in 0..5 {
        assert!(
            plain_get(web, "app.example.com", "/")
                .await
                .starts_with("HTTP/1.1 200")
        );
    }
    let _ = t.server.stop.send(true);
    tokio::time::timeout(Duration::from_secs(12), t.server.task)
        .await
        .expect("the server stops within the drain time")
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_connection_of_another_service_makes_room_on_a_full_client() {
    let (a, _) = recording_app(Behave::Answer).await;
    let (b, _) = recording_app(Behave::Answer).await;
    let secret = Secret::generate();
    let mut s = plain_server(
        &secret,
        &[
            ("a", site(&["home"], a), &["a.example.com"]),
            ("b", site(&["home"], b), &["b.example.com"]),
        ],
    );
    s.max_connections_per_client = 1;
    let web = http(&s);
    let _t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    assert!(
        plain_get(web, "a.example.com", "/")
            .await
            .starts_with("HTTP/1.1 200")
    );
    let got = plain_get(web, "b.example.com", "/").await;
    assert!(
        got.starts_with("HTTP/1.1 200"),
        "the idle one was closed for it: {got}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_kept_visitors_need_no_more_app_connections_than_requests_in_flight() {
    let (app, log) = recording_app(Behave::Answer).await;
    let (s, secret) = plain(app, |_| {});
    let web = http(&s);
    let _t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    let mut all = tokio::task::JoinSet::new();
    for _ in 0..20 {
        all.spawn(async move {
            let mut v = Keep::open(web, localhost()).await;
            for _ in 0..50 {
                assert_eq!(status(&v.get("/").await), 200);
            }
        });
    }
    all.join_all().await;
    assert!(
        log.conns() <= 20,
        "20 visitors with one request each in flight, {} app connections",
        log.conns()
    );
}
