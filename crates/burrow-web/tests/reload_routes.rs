mod common;

use burrow_web::{ProxySettings, Site, SiteOptions};
use common::*;

fn with_paths(site: Site, paths: &[&str]) -> Site {
    Site {
        options: SiteOptions {
            paths: paths.iter().map(|p| p.to_string()).collect(),
            ..SiteOptions::default()
        },
        ..site
    }
}

async fn get(p: &burrow_web::Proxy<Fake>, target: &str, extra: &str) -> String {
    over_tls(
        p.https_addr().unwrap(),
        "example.com",
        &format!("GET {target} HTTP/1.1\r\nHost: example.com\r\n{extra}Connection: close\r\n\r\n"),
    )
    .await
}

fn last_service(up: &Fake) -> String {
    up.visitors.lock().unwrap().last().unwrap().0.clone()
}

#[tokio::test]
async fn routes_and_the_trusted_list_change_together_on_reload() {
    let app = http_backend().await;
    let (up, _) = Fake::new(4);
    up.serve("blog", app.addr);
    up.serve("api", app.addr);
    let proxy = start(
        |s| s.sites = vec![https_site("blog", &["example.com"])],
        up.clone(),
    )
    .await;
    let addr = proxy.https_addr().unwrap();
    let r = get(&proxy, "/api/x", "X-Forwarded-For: 203.0.113.9\r\n").await;
    assert!(r.contains("x-real-ip: 127.0.0.1\n"), "{r}");
    assert_eq!(last_service(&up), "blog");

    let mut next = ProxySettings::new(addr);
    next.sites = vec![
        https_site("blog", &["example.com"]),
        with_paths(https_site("api", &["example.com"]), &["/api"]),
    ];
    next.forwarding.trusted_proxies = vec!["127.0.0.1/32".parse().unwrap()];
    proxy.apply(&next).await;
    let r = get(&proxy, "/api/x", "X-Forwarded-For: 203.0.113.9\r\n").await;
    assert!(r.contains("x-real-ip: 203.0.113.9\n"), "{r}");
    assert_eq!(last_service(&up), "api");
    get(&proxy, "/other", "").await;
    assert_eq!(last_service(&up), "blog");

    next.sites.pop();
    next.forwarding.trusted_proxies.clear();
    proxy.apply(&next).await;
    let r = get(&proxy, "/api/x", "X-Forwarded-For: 203.0.113.9\r\n").await;
    assert!(r.contains("x-real-ip: 127.0.0.1\n"), "{r}");
    assert_eq!(last_service(&up), "blog");
}

#[tokio::test]
async fn a_connection_open_across_a_reload_uses_the_new_routes() {
    let app = http_backend().await;
    let (up, _) = Fake::new(4);
    up.serve("blog", app.addr);
    up.serve("api", app.addr);
    let proxy = start(
        |s| s.sites = vec![https_site("blog", &["example.com"])],
        up.clone(),
    )
    .await;
    let mut t = tls(proxy.https_addr().unwrap(), "example.com")
        .await
        .unwrap();
    let ask = "GET /api HTTP/1.1\r\nHost: example.com\r\n\r\n";
    let r = read_response(&mut t, ask).await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    assert_eq!(last_service(&up), "blog");
    let mut next = ProxySettings::new(proxy.https_addr().unwrap());
    next.sites = vec![
        https_site("blog", &["example.com"]),
        with_paths(https_site("api", &["example.com"]), &["/api"]),
    ];
    proxy.apply(&next).await;
    let r = read_response(&mut t, ask).await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    assert_eq!(last_service(&up), "api");
}
