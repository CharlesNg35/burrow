use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use burrow::config::settings::ServerSettings;
use burrow::wire::secret::Secret;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
    let mut v = Keep::open(web).await;
    assert_eq!(status(&v.get("/").await), 200);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut v = Keep::open(web).await;
    let got = v.send("GET", "/", "x-id: get\r\n").await.unwrap();
    assert_eq!(status(&got), 200, "{got}");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let got = v.send("POST", "/", "x-id: post\r\n").await.unwrap();
    assert_eq!(status(&got), 200, "{got}");
    assert_eq!(seen(&log, "get"), 1);
    assert_eq!(seen(&log, "post"), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_get_lost_on_a_connection_kept_idle_is_sent_again_once_and_a_post_never() {
    let (app, log) = recording_app(Behave::CloseOnReuse).await;
    let (s, secret) = plain(app, |_| {});
    let web = http(&s);
    let _t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    let mut v = Keep::open(web).await;
    assert_eq!(
        status(&v.send("GET", "/", "x-id: a\r\n").await.unwrap()),
        200
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let got = v.send("GET", "/", "x-id: b\r\n").await.unwrap();
    assert_eq!(status(&got), 200, "sent again on a new connection: {got}");
    assert_eq!(
        seen(&log, "b"),
        2,
        "the app saw it twice, as Go and nginx allow"
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
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
    let mut v = Keep::open(web).await;
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
        let mut v = Keep::open(web).await;
        for i in 0..15 {
            let method = if i % 3 == 0 { "POST" } else { "GET" };
            let id = format!("{round}-{i}");
            methods.insert(id.clone(), method);
            if v.send(method, "/", &format!("x-id: {id}\r\n"))
                .await
                .is_none()
            {
                v = Keep::open(web).await;
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
    let mut a = Keep::open(web).await;
    assert_eq!(status(&a.get("/").await), 401);
    let mut b = Keep::open(web).await;
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
            let mut v = Keep::open(web).await;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ntlm_connection_stays_with_its_visitor_behind_a_proxy_that_carries_several() {
    let (app, log) = recording_app(Behave::Ntlm).await;
    let (s, secret) = plain(app, |s| {
        s.web.as_mut().unwrap().trusted_proxies = vec!["127.0.0.1/32".parse().unwrap()];
    });
    let web = http(&s);
    let _t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    let mut proxy = Keep::open(web).await;
    let a = "X-Forwarded-For: 203.0.113.1\r\n";
    let b = "X-Forwarded-For: 203.0.113.2\r\n";
    assert_eq!(status(&proxy.send("GET", "/", a).await.unwrap()), 401);
    assert_eq!(status(&proxy.send("GET", "/", b).await.unwrap()), 401);
    let step = format!("{a}Authorization: NTLM step2\r\n");
    assert_eq!(status(&proxy.send("GET", "/", &step).await.unwrap()), 200);
    let asked = log.asked();
    let of = |ip: &str| -> Vec<usize> {
        asked
            .iter()
            .filter(|r| r.real_ip == ip.parse().ok())
            .map(|r| r.conn)
            .collect()
    };
    let (conns_a, conns_b) = (of("203.0.113.1"), of("203.0.113.2"));
    assert_eq!(conns_a.len(), 2);
    assert_eq!(
        conns_a[0], conns_a[1],
        "A's handshake stays on its own connection"
    );
    assert!(
        !conns_a.contains(&conns_b[0]),
        "B never gets A's authenticated connection: {asked:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_get_is_not_sent_again_once_part_of_its_answer_arrived() {
    let (app, log) = recording_app(Behave::HalfHeadOnReuse).await;
    let (s, secret) = plain(app, |_| {});
    let web = http(&s);
    let _t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    let mut v = Keep::open(web).await;
    let first = v.send("GET", "/", "x-id: a\r\n").await.unwrap();
    assert_eq!(status(&first), 200);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let got = v.send("GET", "/", "x-id: b\r\n").await.unwrap();
    assert_eq!(status(&got), 502, "{got}");
    assert_eq!(
        seen(&log, "b"),
        1,
        "an answer had started, so it is not sent again"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_visitor_lingering_after_an_early_answer_holds_no_app_connection() {
    let (app, _log) = recording_app(Behave::Answer).await;
    let (s, secret) = plain(app, |s| {
        s.services.get_mut("app").unwrap().max_connections = Some(1);
        let site = s.web.as_mut().unwrap().sites.get_mut("app").unwrap();
        site.options.max_body_size = Some(16);
    });
    let web = http(&s);
    let _t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    let mut lingering = tokio::net::TcpStream::connect(web).await.unwrap();
    let warm = b"GET / HTTP/1.1\r\nHost: app.example.com\r\n\r\n";
    lingering.write_all(warm).await.unwrap();
    let mut buf = vec![0u8; 4096];
    let n = lingering.read(&mut buf).await.unwrap();
    assert!(buf[..n].starts_with(b"HTTP/1.1 200"));
    let big = b"POST / HTTP/1.1\r\nHost: app.example.com\r\nContent-Length: 100000\r\n\r\n";
    lingering.write_all(big).await.unwrap();
    let n = lingering.read(&mut buf).await.unwrap();
    assert!(
        buf[..n].starts_with(b"HTTP/1.1 413"),
        "{}",
        String::from_utf8_lossy(&buf[..n])
    );
    let other = plain_get(web, "app.example.com", "/").await;
    assert!(
        other.starts_with("HTTP/1.1 200"),
        "another visitor is served while the first lingers: {other}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_that_closes_after_every_answer_gives_no_bad_gateway_and_no_request_twice() {
    let (app, log) = recording_app(Behave::OneAnswerPerConnection).await;
    let (s, secret) = plain(app, |_| {});
    let web = http(&s);
    let _t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    let mut bad = Vec::new();
    for i in 0..60 {
        let mut v = Keep::open(web).await;
        let got = v
            .send("GET", "/", &format!("x-id: {i}\r\n"))
            .await
            .unwrap_or_default();
        if status(&got) != 200 {
            bad.push((i, got));
        }
    }
    assert!(bad.is_empty(), "{} of 60 failed: {bad:?}", bad.len());
    for i in 0..60 {
        assert_eq!(seen(&log, &i.to_string()), 1, "request {i}");
    }
}
