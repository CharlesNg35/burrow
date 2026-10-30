use std::time::Duration;

use burrow::config::settings::{ClientAuth, WebSite};
use burrow::wire::secret::Secret;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use crate::common::*;
use crate::kit::*;

async fn no_open(t: &Tunnel, service: &str) {
    let shared = t.server.shared.clone();
    let service = service.to_string();
    wait_for(move || {
        let status = shared.status();
        shared.active_connections() == 0
            && status
                .services
                .iter()
                .all(|s| s.name != service || s.open == 0)
    })
    .await;
}

async fn hello_then_silence(web: std::net::SocketAddr, name: &str) -> TcpStream {
    let mut s = TcpStream::connect(web).await.unwrap();
    s.write_all(&client_hello(name)).await.unwrap();
    s
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_silent_visitor_whose_client_refuses_is_reset_and_not_kept() {
    let app = counting_backend().await;
    let secret = Secret::generate();
    let s = web_server(
        &secret,
        &[("nas", site(&["home"], app), &["nas.example.com"])],
    );
    let t = tunnel_with(
        s,
        Vec::new(),
        |mut c| {
            c.allow =
                burrow::policy::allowlist::AllowList::Rules(vec!["10.0.0.0/8".parse().unwrap()]);
            c
        },
        secret,
    )
    .await;
    let web = t.server.web_addr.unwrap();
    let mut silent = hello_then_silence(web, "nas.example.com").await;
    assert!(
        reset_quickly(&mut silent).await,
        "the refused visit resets the visitor"
    );
    no_open(&t, "nas").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn silent_visitors_over_the_clients_limit_are_reset_every_time() {
    let app = counting_backend().await;
    let secret = Secret::generate();
    let mut s = web_server(
        &secret,
        &[("nas", site(&["home"], app), &["nas.example.com"])],
    );
    s.clients.insert(
        "home".into(),
        ClientAuth {
            secrets: vec![secret.clone()],
            max_connections: Some(1),
        },
    );
    let t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    let web = t.server.web_addr.unwrap();
    let mut held = hello_then_silence(web, "nas.example.com").await;
    let shared = t.server.shared.clone();
    wait_for(|| shared.active_connections() == 1).await;
    for i in 0..30 {
        let mut silent = hello_then_silence(web, "nas.example.com").await;
        assert!(
            reset_quickly(&mut silent).await,
            "visitor {i} over the limit is reset"
        );
    }
    held.shutdown().await.unwrap();
    assert!(clean_answer(&mut held).await.starts_with("got "));
    no_open(&t, "nas").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_removed_host_resets_its_silent_visitor_and_a_kept_one_flows_on() {
    let app = counting_backend().await;
    let secret = Secret::generate();
    let s = web_server(
        &secret,
        &[
            ("nas", site(&["home"], app), &["nas.example.com"]),
            ("git", site(&["home"], app), &["git.example.com"]),
        ],
    );
    let t = tunnel_with(s.clone(), Vec::new(), |c| c, secret).await;
    let web = t.server.web_addr.unwrap();
    let mut removed = hello_then_silence(web, "git.example.com").await;
    let mut kept = hello_then_silence(web, "nas.example.com").await;
    let shared = t.server.shared.clone();
    wait_for(|| shared.active_connections() == 2).await;
    let mut next = s.clone();
    let web_settings = next.web.as_mut().unwrap();
    web_settings.sites.remove("git");
    web_settings.sites.insert(
        "nas".into(),
        WebSite::passthrough(vec!["nas.example.com".into(), "files.example.com".into()]),
    );
    t.server.reload_applied(next.clone()).await;
    assert!(
        reset_quickly(&mut removed).await,
        "a removed host's visitor is reset"
    );
    kept.write_all(b"more after the reload").await.unwrap();
    kept.shutdown().await.unwrap();
    let answer = clean_answer(&mut kept).await;
    let sent = client_hello("nas.example.com").len() + b"more after the reload".len();
    assert_eq!(
        answer,
        format!("got {sent}"),
        "the kept site ends with a normal close"
    );
    no_open(&t, "nas").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_half_close_gets_the_whole_answer_and_a_normal_close() {
    let app = counting_backend().await;
    let secret = Secret::generate();
    let s = web_server(
        &secret,
        &[("nas", site(&["home"], app), &["nas.example.com"])],
    );
    let t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    let web = t.server.web_addr.unwrap();
    for _ in 0..5 {
        let mut s = hello_then_silence(web, "nas.example.com").await;
        s.shutdown().await.unwrap();
        let answer = clean_answer(&mut s).await;
        assert_eq!(
            answer,
            format!("got {}", client_hello("nas.example.com").len())
        );
    }
    no_open(&t, "nas").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shutdown_lets_a_connection_that_ends_in_time_close_normally() {
    let app = counting_backend().await;
    let secret = Secret::generate();
    let s = web_server(
        &secret,
        &[("nas", site(&["home"], app), &["nas.example.com"])],
    );
    let t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    let web = t.server.web_addr.unwrap();
    let mut s = hello_then_silence(web, "nas.example.com").await;
    let shared = t.server.shared.clone();
    wait_for(|| shared.active_connections() == 1).await;
    t.server.stop.send(true).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    s.shutdown().await.unwrap();
    let answer = clean_answer(&mut s).await;
    assert!(answer.starts_with("got "), "{answer}");
    let done = tokio::time::timeout(Duration::from_secs(5), t.server.task).await;
    assert!(done.is_ok(), "the drain ends once the connection closed");
}
