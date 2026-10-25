use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering};
use std::time::Duration;

use burrow::config::settings::{Health, HttpCheck, WebSettings, WebSite, WebTls};
use burrow_config::Mode;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::common::*;
use super::{Farm, web};

const HOST: &str = "app.example.com";

async fn http_app(addr: SocketAddr, tag: char, code: Arc<AtomicU16>) -> Arc<AtomicUsize> {
    let l = TcpListener::bind(addr).await.unwrap();
    let asked = Arc::new(AtomicUsize::new(0));
    let count = asked.clone();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            let (code, count) = (code.clone(), count.clone());
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut one = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    match s.read(&mut one).await {
                        Ok(1) => head.push(one[0]),
                        _ => return,
                    }
                }
                let reply = format!(
                    "HTTP/1.1 {} X\r\nContent-Length: 1\r\n\r\n{tag}",
                    code.load(Ordering::SeqCst)
                );
                let _ = s.write_all(reply.as_bytes()).await;
                let head = String::from_utf8_lossy(&head).to_ascii_lowercase();
                if head.contains("\r\nuser-agent: burrow-health-check/") {
                    count.fetch_add(1, Ordering::SeqCst);
                }
            });
        }
    });
    asked
}

fn checked() -> Option<Health> {
    Some(Health {
        interval: Duration::from_millis(200),
        timeout: Duration::from_secs(1),
        failures: 2,
        http: Some(HttpCheck {
            path: "/healthz".into(),
            host: HOST.into(),
        }),
    })
}

async fn served_by(http: SocketAddr) -> Option<char> {
    let mut s = TcpStream::connect(http).await.ok()?;
    let req = format!("GET / HTTP/1.1\r\nHost: {HOST}\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.ok()?;
    let mut got = String::new();
    tokio::time::timeout(Duration::from_secs(5), s.read_to_string(&mut got))
        .await
        .ok()?
        .ok()?;
    got.chars().last()
}

fn health(farm: &Farm) -> Vec<(String, bool, Option<String>)> {
    farm.server.shared.status().services[0]
        .health
        .iter()
        .map(|h| (h.client.clone(), h.up, h.reason.clone()))
        .collect()
}

async fn wait_health(farm: &Farm, want: &[(&str, bool)]) {
    for _ in 0..100 {
        let got = health(farm);
        let shape: Vec<(&str, bool)> = got.iter().map(|(c, up, _)| (c.as_str(), *up)).collect();
        if shape == want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("health never became {want:?}: {:?}", health(farm));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_answering_500_moves_new_requests_to_the_other_client_and_back_after_one_200() {
    let http: SocketAddr = format!("127.0.0.1:{}", free_port()).parse().unwrap();
    let mut farm = Farm::start(web(Mode::Failover, checked()), |s| {
        let mut w = WebSettings::on((None, Some(http)));
        let mut site = WebSite::passthrough(vec![HOST.into()]);
        site.tls = WebTls::Off;
        site.https_redirect = false;
        w.sites.insert("web".into(), site);
        s.web = Some(w);
    })
    .await;
    let codes = [Arc::new(AtomicU16::new(200)), Arc::new(AtomicU16::new(200))];
    let mut asked = Vec::new();
    for (i, tag) in [(0, 'A'), (1, 'B')] {
        farm.stop_target(i).await;
        asked.push(http_app(farm.sites[i].target, tag, codes[i].clone()).await);
    }
    wait_for(|| asked.iter().all(|n| n.load(Ordering::SeqCst) >= 2)).await;
    wait_health(&farm, &[("a", true), ("b", true)]).await;
    assert_eq!(served_by(http).await, Some('A'));
    codes[0].store(500, Ordering::SeqCst);
    wait_health(&farm, &[("a", false), ("b", true)]).await;
    let (_, _, why) = &health(&farm)[0];
    assert_eq!(why.as_deref(), Some("answered 500 Internal Server Error"));
    assert!(
        farm.server.shared.status().services[0].set_aside.is_empty(),
        "a client that reaches its app is not set aside"
    );
    assert_eq!(served_by(http).await, Some('B'));
    codes[1].store(503, Ordering::SeqCst);
    wait_health(&farm, &[("a", false), ("b", false)]).await;
    assert!(
        served_by(http).await.is_some(),
        "with every client down, a connected one is still tried"
    );
    codes[0].store(200, Ordering::SeqCst);
    wait_health(&farm, &[("a", true), ("b", false)]).await;
    assert_eq!(health(&farm)[0].2, None);
    assert_eq!(served_by(http).await, Some('A'));
}
