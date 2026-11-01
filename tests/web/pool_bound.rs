use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use burrow::wire::secret::Secret;
use burrow_config::ProxyVersion;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

use crate::common::*;
use crate::kit::*;
use crate::pool_kit::*;

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_a_proxy_header_each_request_arrives_on_a_connection_that_names_its_own_visitor() {
    let (app, log) = recording_app(Behave::Answer).await;
    let secret = Secret::generate();
    let mut svc = site(&["home"], app);
    svc.proxy_protocol = Some(ProxyVersion::V1);
    let s = plain_server(&secret, &[("app", svc, &["app.example.com"])]);
    let web = s.web.as_ref().unwrap().http.unwrap();
    let _t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    let mut a = Keep::open(web, ip("127.0.0.2")).await;
    let mut b = Keep::open(web, ip("127.0.0.3")).await;
    for _ in 0..6 {
        assert_eq!(status(&a.get("/").await), 200);
        assert_eq!(status(&b.get("/").await), 200);
        let mut fresh = Keep::open(web, ip("127.0.0.3")).await;
        assert_eq!(status(&fresh.get("/").await), 200);
    }
    let asked = log.asked();
    assert_eq!(asked.len(), 18);
    for r in &asked {
        assert!(r.proxy_source.is_some(), "{r:?}");
        assert_eq!(r.proxy_source, r.real_ip, "{r:?}");
    }
    let conns_of = |who: IpAddr| {
        let mut c: Vec<usize> = asked
            .iter()
            .filter(|r| r.real_ip == Some(who))
            .map(|r| r.conn)
            .collect();
        c.dedup();
        c
    };
    assert_eq!(
        conns_of(ip("127.0.0.2")).len(),
        1,
        "a kept visitor reuses its own"
    );
}

async fn slow_proxy_app() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            tokio::spawn(async move {
                let mut first = [0u8; 6];
                if s.peek(&mut first).await.is_ok() && &first == b"PROXY " {
                    let _ = read_line_raw(&mut s).await;
                }
                while read_head(&mut s).await.is_some() {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    let ok = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
                    if s.write_all(ok).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    addr
}

async fn read_line_raw(s: &mut tokio::net::TcpStream) -> Option<()> {
    use tokio::io::AsyncReadExt;
    let mut one = [0u8; 1];
    let mut last = 0u8;
    loop {
        if s.read(&mut one).await.ok()? != 1 {
            return None;
        }
        if last == b'\r' && one[0] == b'\n' {
            return Some(());
        }
        last = one[0];
    }
}

async fn burst(web: SocketAddr, from: IpAddr, n: usize) -> Vec<u16> {
    let mut all = tokio::task::JoinSet::new();
    for _ in 0..n {
        all.spawn(async move {
            let mut v = Keep::open(web, from).await;
            status(&v.get("/").await)
        });
    }
    all.join_all().await
}

async fn second_visitor_after_a_burst_with_proxy(on_client: bool) {
    let secret = Secret::generate();
    let mut svc = site(&["home"], slow_proxy_app().await);
    svc.proxy_protocol = Some(ProxyVersion::V1);
    if !on_client {
        svc.max_connections = Some(30);
    }
    let mut s = plain_server(&secret, &[("app", svc, &["app.example.com"])]);
    if on_client {
        s.max_connections_per_client = 30;
    }
    let web = s.web.as_ref().unwrap().http.unwrap();
    let _t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    let got = burst(web, ip("127.0.0.2"), 24).await;
    assert!(got.iter().all(|s| *s == 200), "{got:?}");
    tokio::time::sleep(Duration::from_secs(1)).await;
    let got = burst(web, ip("127.0.0.3"), 20).await;
    assert!(
        got.iter().all(|s| *s == 200),
        "idle connections bound to the first visitor were closed for the second: {got:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_proxy_bound_connections_make_room_under_the_services_limit() {
    second_visitor_after_a_burst_with_proxy(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_proxy_bound_connections_make_room_under_the_clients_limit() {
    second_visitor_after_a_burst_with_proxy(true).await;
}
