use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use burrow::config::settings::ServerSettings;
use burrow::wire::secret::Secret;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::common::*;
use crate::kit::*;

const POOL: usize = 16;
const ATTACKER: &str = "203.0.113.7";

type Holding = Arc<AtomicUsize>;

async fn holding_app() -> (SocketAddr, Holding) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let holding = Holding::default();
    let count = holding.clone();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            let count = count.clone();
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut one = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    match s.read(&mut one).await {
                        Ok(1) => head.push(one[0]),
                        _ => return,
                    }
                }
                if head.starts_with(b"GET /hold ") {
                    count.fetch_add(1, Ordering::SeqCst);
                    let _ = s.read(&mut one).await;
                    count.fetch_sub(1, Ordering::SeqCst);
                    return;
                }
                let _ = s
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .await;
            });
        }
    });
    (addr, holding)
}

fn behind_a_proxy(s: &mut ServerSettings) -> SocketAddr {
    let web = s.web.as_mut().unwrap();
    web.trusted_proxies = vec!["127.0.0.1/32".parse().unwrap()];
    web.http.unwrap()
}

fn request(path: &str, from: Option<&str>) -> String {
    let forwarded = from.map_or(String::new(), |f| format!("X-Forwarded-For: {f}\r\n"));
    format!("GET {path} HTTP/1.1\r\nHost: app.example.com\r\n{forwarded}Connection: close\r\n\r\n")
}

async fn status(http: SocketAddr, from: Option<&str>) -> String {
    let mut s = TcpStream::connect(http).await.unwrap();
    s.write_all(request("/", from).as_bytes()).await.unwrap();
    let answer = clean_answer(&mut s).await;
    answer.lines().next().unwrap_or_default().to_string()
}

async fn hold(
    http: SocketAddr,
    from: &str,
    holding: &Holding,
    before: usize,
) -> Result<TcpStream, String> {
    let mut s = TcpStream::connect(http).await.unwrap();
    s.write_all(request("/hold", Some(from)).as_bytes())
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut got = [0u8; 64];
    loop {
        if holding.load(Ordering::SeqCst) > before {
            return Ok(s);
        }
        if let Ok(read) = tokio::time::timeout(Duration::from_millis(20), s.read(&mut got)).await {
            let n = read.unwrap_or(0);
            return Err(String::from_utf8_lossy(&got[..n]).into_owned());
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "neither held nor answered"
        );
    }
}

async fn flood(http: SocketAddr, holding: &Holding) -> (Vec<TcpStream>, Vec<String>) {
    let (mut held, mut refused) = (Vec::new(), Vec::new());
    for _ in 0..POOL {
        match hold(http, ATTACKER, holding, held.len()).await {
            Ok(s) => held.push(s),
            Err(answer) => refused.push(answer),
        }
    }
    (held, refused)
}

async fn one_address_cannot_fill(pool_on_client: bool) {
    let (app, holding) = holding_app().await;
    let secret = Secret::generate();
    let mut svc = site(&["home"], app);
    if !pool_on_client {
        svc.max_connections = Some(POOL);
    }
    let mut s = plain_server(&secret, &[("app", svc, &["app.example.com"])]);
    if pool_on_client {
        s.max_connections_per_client = POOL;
    }
    let http = behind_a_proxy(&mut s);
    let t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    let (held, refused) = flood(http, &holding).await;
    assert_eq!(
        held.len(),
        POOL / 2 + 1,
        "the address keeps every connection up to half the pool, and its part past it"
    );
    assert_eq!(refused.len(), POOL - held.len());
    assert!(
        refused.iter().all(|r| r.starts_with("HTTP/1.1 503")),
        "{refused:?}"
    );
    assert_eq!(status(http, Some("203.0.113.8")).await, "HTTP/1.1 200 OK");
    assert_eq!(status(http, Some("2001:db8::1")).await, "HTTP/1.1 200 OK");
    assert_eq!(status(http, None).await, "HTTP/1.1 200 OK");
    assert!(
        status(http, Some(ATTACKER))
            .await
            .starts_with("HTTP/1.1 503")
    );
    drop(held);
    let open = t.server.shared.clone();
    wait_for(|| open.active_connections() == 0).await;
    assert_eq!(status(http, Some(ATTACKER)).await, "HTTP/1.1 200 OK");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_visitor_address_cannot_fill_a_services_connections() {
    one_address_cannot_fill(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_visitor_address_cannot_fill_a_clients_connections() {
    one_address_cannot_fill(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn several_real_clients_behind_one_proxy_each_get_their_part() {
    let (app, holding) = holding_app().await;
    let secret = Secret::generate();
    let mut svc = site(&["home"], app);
    svc.max_connections = Some(POOL);
    let mut s = plain_server(&secret, &[("app", svc, &["app.example.com"])]);
    let http = behind_a_proxy(&mut s);
    let _t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    let mut held = Vec::new();
    for from in ["198.51.100.1", "198.51.100.2"] {
        for _ in 0..7 {
            let before = held.len();
            held.push(hold(http, from, &holding, before).await.expect("held"));
        }
    }
    assert_eq!(held.len(), 14, "both real clients keep all they asked for");
}
