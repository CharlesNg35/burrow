use std::collections::BTreeSet;
use std::net::SocketAddr;

use burrow::config::settings::{DB_IP_FILE, DB_IP_FOLDER, GeoSource, ServerSettings};
use burrow::policy::geo::Countries;
use burrow::wire::secret::Secret;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use crate::common::geo::{now, world};
use crate::common::*;
use crate::kit::*;

fn codes(list: &[&str]) -> BTreeSet<String> {
    list.iter().map(|c| c.to_string()).collect()
}

async fn shop(countries: Countries, geoip: GeoSource) -> (Tunnel, SocketAddr) {
    let app = http_app().await;
    let secret = Secret::generate();
    let mut svc = site(&["home"], app);
    svc.countries = Some(countries);
    let mut s: ServerSettings = plain_server(&secret, &[("shop", svc, &["shop.example.com"])]);
    s.geoip = Some(geoip);
    let web = s.web.as_mut().unwrap();
    web.trusted_proxies = vec!["127.0.0.1/32".parse().unwrap()];
    let http = web.http.unwrap();
    let t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    (t, http)
}

async fn answer(http: SocketAddr, from: Option<&str>) -> String {
    let forwarded = from.map_or(String::new(), |f| format!("X-Forwarded-For: {f}\r\n"));
    let mut s = TcpStream::connect(http).await.unwrap();
    let req =
        format!("GET / HTTP/1.1\r\nHost: shop.example.com\r\n{forwarded}Connection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    clean_answer(&mut s).await
}

fn head(answer: &str) -> &str {
    answer.lines().next().unwrap_or_default()
}

async fn ready(t: &Tunnel) {
    let shared = t.server.shared.clone();
    wait_for(|| shared.status().geoip.is_some_and(|g| g.ready())).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_website_with_allow_countries_lets_in_only_those_countries() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("own.mmdb");
    std::fs::write(&file, world(now())).unwrap();
    let (t, http) = shop(Countries::Allow(codes(&["FR"])), GeoSource::File(file)).await;
    ready(&t).await;
    assert_eq!(
        head(&answer(http, Some("203.0.113.9")).await),
        "HTTP/1.1 200 OK"
    );
    let refused = answer(http, Some("198.51.100.4")).await;
    assert!(head(&refused).starts_with("HTTP/1.1 403"), "{refused}");
    assert!(
        refused.contains("isn't available in your region"),
        "{refused}"
    );
    assert!(
        !refused.contains("db-ip.com"),
        "an own file is not DB-IP's: {refused}"
    );
    let unknown = answer(http, Some("192.0.2.1")).await;
    assert!(head(&unknown).starts_with("HTTP/1.1 403"), "{unknown}");
    assert_eq!(
        head(&answer(http, None).await),
        "HTTP/1.1 200 OK",
        "a loopback visitor is not judged by country"
    );
    assert_eq!(
        head(&answer(http, Some("10.1.2.3")).await),
        "HTTP/1.1 200 OK"
    );
    let status = t.server.shared.status();
    let shop = status.services.iter().find(|s| s.name == "shop").unwrap();
    assert_eq!(shop.refused_country, 2);
    assert_eq!(shop.allowed_countries, ["FR"]);
    assert!(shop.denied_countries.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_db_ip_file_in_the_state_folder_is_used_and_credited_without_a_download() {
    let state = tempfile::tempdir().unwrap();
    let dir = state.path().join(DB_IP_FOLDER);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(DB_IP_FILE), world(now())).unwrap();
    let (t, http) = shop(
        Countries::Deny(codes(&["US"])),
        GeoSource::DbIp { dir: Some(dir) },
    )
    .await;
    ready(&t).await;
    let refused = answer(http, Some("198.51.100.4")).await;
    assert!(head(&refused).starts_with("HTTP/1.1 403"), "{refused}");
    assert!(
        refused.contains("<a href=\"https://db-ip.com\">IP Geolocation by DB-IP</a>"),
        "{refused}"
    );
    assert_eq!(
        head(&answer(http, Some("203.0.113.9")).await),
        "HTTP/1.1 200 OK"
    );
    assert_eq!(
        head(&answer(http, Some("192.0.2.1")).await),
        "HTTP/1.1 200 OK",
        "deny_countries lets unknown addresses in"
    );
    let g = t.server.shared.status().geoip.unwrap();
    assert!(g.credit && !g.stale && !g.failed, "{g:?}");
    let shared = t.server.shared.clone();
    wait_for(|| {
        shared
            .status()
            .geoip
            .is_some_and(|g| g.result.starts_with("up to date"))
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_a_database_allow_countries_refuses_public_visitors() {
    let missing = tempfile::tempdir().unwrap().path().join("none.mmdb");
    let (t, http) = shop(Countries::Allow(codes(&["FR"])), GeoSource::File(missing)).await;
    let refused = answer(http, Some("203.0.113.9")).await;
    assert!(
        head(&refused).starts_with("HTTP/1.1 403"),
        "fails closed: {refused}"
    );
    assert_eq!(
        head(&answer(http, Some("10.0.0.9")).await),
        "HTTP/1.1 200 OK"
    );
    let g = t.server.shared.status().geoip.unwrap();
    assert!(!g.ready(), "{g:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_country_database_comes_and_goes_with_geoip_on_reload() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("own.mmdb");
    std::fs::write(&file, world(now())).unwrap();
    let app = http_app().await;
    let secret = Secret::generate();
    let mut svc = site(&["home"], app);
    svc.countries = Some(Countries::Allow(codes(&["FR"])));
    let mut s = plain_server(&secret, &[("shop", svc, &["shop.example.com"])]);
    let web = s.web.as_mut().unwrap();
    web.trusted_proxies = vec!["127.0.0.1/32".parse().unwrap()];
    let http = web.http.unwrap();
    let t = tunnel_with(s.clone(), Vec::new(), |c| c, secret).await;
    assert!(
        t.server.shared.status().geoip.is_none(),
        "no geoip, no database task"
    );
    let mut with = s.clone();
    with.geoip = Some(GeoSource::File(file));
    t.server.reload_applied(with).await;
    ready(&t).await;
    assert!(head(&answer(http, Some("198.51.100.4")).await).starts_with("HTTP/1.1 403"));
    assert_eq!(
        head(&answer(http, Some("203.0.113.9")).await),
        "HTTP/1.1 200 OK"
    );
    t.server.reload_applied(s).await;
    assert!(
        t.server.shared.status().geoip.is_none(),
        "the task stops with geoip gone"
    );
}
