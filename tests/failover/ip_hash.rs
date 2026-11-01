use std::net::SocketAddr;

use burrow_config::Mode;

use super::{Farm, eventually_served_by, quick_health, served_by, web};

async fn first_choice(addr: SocketAddr) -> char {
    let tag = served_by(addr).await.expect("served");
    for _ in 0..10 {
        assert_eq!(served_by(addr).await, Some(tag), "one address, one client");
    }
    tag
}

fn other(tag: char) -> char {
    if tag == 'A' { 'B' } else { 'A' }
}

fn index_of(tag: char) -> usize {
    usize::from(tag == 'B')
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tcp_visitor_keeps_its_client_and_moves_only_while_it_is_unhealthy() {
    let mut farm = Farm::start(web(Mode::IpHash, quick_health()), |_| {}).await;
    let addr = farm.server.services["web"];
    let mine = first_choice(addr).await;
    let i = index_of(mine);
    farm.stop_target(i).await;
    eventually_served_by(addr, other(mine)).await;
    for _ in 0..5 {
        assert_eq!(served_by(addr).await, Some(other(mine)));
    }
    farm.restart_target(i).await;
    eventually_served_by(addr, mine).await;
    assert_eq!(first_choice(addr).await, mine, "back on its own client");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tcp_visitor_moves_while_its_client_is_offline_and_returns() {
    let mut farm = Farm::start(web(Mode::IpHash, None), |_| {}).await;
    let addr = farm.server.services["web"];
    let mine = first_choice(addr).await;
    let i = index_of(mine);
    farm.disconnect(i).await;
    assert_eq!(first_choice(addr).await, other(mine));
    farm.connect(i).await;
    assert_eq!(first_choice(addr).await, mine);
}

#[cfg(feature = "web")]
mod web_sites {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU16;
    use std::time::Duration;

    use burrow::config::settings::ServerSettings;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    use super::super::common::*;
    use super::super::http_check::{HOST, http_app, plain_site};
    use super::super::weights::weigh;
    use super::*;

    async fn asked_from(http: SocketAddr, visitor: &str) -> Option<char> {
        let mut s = TcpStream::connect(http).await.ok()?;
        let req = format!(
            "GET / HTTP/1.1\r\nHost: {HOST}\r\nX-Forwarded-For: {visitor}\r\nConnection: close\r\n\r\n"
        );
        s.write_all(req.as_bytes()).await.ok()?;
        let mut got = String::new();
        tokio::time::timeout(Duration::from_secs(5), s.read_to_string(&mut got))
            .await
            .ok()?
            .ok()?;
        got.chars().last()
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Front {
        Direct,
        TrustedProxy,
        ProxyHeader,
    }

    async fn proxied_from(http: SocketAddr, visitor: &str) -> Option<char> {
        let mut s = TcpStream::connect(http).await.ok()?;
        let src = SocketAddr::new(visitor.parse().ok()?, 40000);
        let head = burrow::wire::proxy_protocol::header(burrow_config::ProxyVersion::V1, src, http);
        s.write_all(&head).await.ok()?;
        let req = format!("GET / HTTP/1.1\r\nHost: {HOST}\r\nConnection: close\r\n\r\n");
        s.write_all(req.as_bytes()).await.ok()?;
        let mut got = String::new();
        tokio::time::timeout(Duration::from_secs(5), s.read_to_string(&mut got))
            .await
            .ok()?
            .ok()?;
        got.chars().last()
    }

    async fn web_farm(svc: burrow::config::settings::Service, front: Front) -> (Farm, SocketAddr) {
        let http: SocketAddr = format!("127.0.0.1:{}", free_port()).parse().unwrap();
        let mut farm = Farm::start(svc, |s: &mut ServerSettings| {
            plain_site(s, http);
            let web = s.web.as_mut().unwrap();
            let local = vec!["127.0.0.1/32".parse().unwrap()];
            match front {
                Front::Direct => {}
                Front::TrustedProxy => web.trusted_proxies = local,
                Front::ProxyHeader => web.proxy_protocol_from = local,
            }
        })
        .await;
        for (i, tag) in [(0, 'A'), (1, 'B')] {
            farm.stop_target(i).await;
            http_app(farm.sites[i].target, tag, Arc::new(AtomicU16::new(200))).await;
        }
        (farm, http)
    }

    fn visitors(n: u32) -> Vec<String> {
        (0..n)
            .map(|i| format!("198.51.{}.{}", i / 200, 1 + i % 200))
            .collect()
    }

    async fn spread(http: SocketAddr, n: u32) -> BTreeMap<char, u32> {
        let mut seen = BTreeMap::new();
        for v in visitors(n) {
            let first = asked_from(http, &v).await.expect("answered");
            for _ in 0..2 {
                assert_eq!(asked_from(http, &v).await, Some(first), "{v} moved");
            }
            *seen.entry(first).or_insert(0) += 1;
        }
        seen
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn each_real_visitor_behind_a_trusted_proxy_keeps_one_client_and_weights_set_the_split() {
        let mut svc = web(Mode::IpHash, None);
        weigh(&mut svc, 3);
        let (_farm, http) = web_farm(svc, Front::TrustedProxy).await;
        let seen = spread(http, 120).await;
        let a = seen.get(&'A').copied().unwrap_or(0);
        assert!((75..=105).contains(&a), "weight 3 to 1: {seen:?}");
        assert_eq!(seen.values().sum::<u32>(), 120, "{seen:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_untrusted_forwarded_for_is_ignored_so_the_peer_address_decides() {
        let (_farm, http) = web_farm(web(Mode::IpHash, None), Front::Direct).await;
        let seen = spread(http, 20).await;
        assert_eq!(
            seen.len(),
            1,
            "every request comes from 127.0.0.1: {seen:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_address_in_a_proxy_header_from_a_listed_balancer_decides_the_client() {
        let (_farm, http) = web_farm(web(Mode::IpHash, None), Front::ProxyHeader).await;
        let mut seen = BTreeMap::new();
        for v in visitors(40) {
            let first = proxied_from(http, &v).await.expect("answered");
            for _ in 0..2 {
                assert_eq!(proxied_from(http, &v).await, Some(first), "{v} moved");
            }
            *seen.entry(first).or_insert(0) += 1;
        }
        assert_eq!(
            seen.len(),
            2,
            "the PROXY address, not 127.0.0.1, decides: {seen:?}"
        );
    }
}
