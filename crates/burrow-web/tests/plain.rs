mod common;

use std::sync::Arc;

use burrow_web::tls::CertState;
use common::*;

struct Setup {
    proxy: burrow_web::Proxy<Fake>,
    certs: Arc<Scripted>,
    app: HttpBackend,
}

async fn setup() -> Setup {
    let app = http_backend().await;
    let (up, _) = Fake::new(4);
    for name in ["wiki", "blog", "shop", "nas"] {
        up.serve(name, app.addr);
    }
    let certs = Scripted::new();
    let proxy = start_with(
        |s| {
            s.sites = vec![
                plain_site("wiki", &["wiki.lan.example.com", "nas"]),
                burrow_web::Site {
                    https_redirect: false,
                    ..https_site("blog", &["blog.example.com"])
                },
                https_site("shop", &["shop.example.com", "*.shop.example.com"]),
                site("nas", &["files.example.com"]),
            ];
            s.tunnel = Some(burrow_web::TunnelSettings {
                host: "tunnel.example.com".into(),
                path: "/burrow".into(),
                mode: burrow_web::TunnelMode::OwnName,
                port: burrow_web::TunnelPort::Https,
            });
        },
        up,
        certs.clone(),
        true,
    )
    .await;
    Setup { proxy, certs, app }
}

fn get(host: &str, target: &str) -> String {
    format!("GET {target} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n")
}

#[tokio::test]
async fn plain_sites_and_https_sites_that_also_serve_plain_http_reach_their_app_on_port_80() {
    let s = setup().await;
    let http = s.proxy.http_addr().unwrap();
    for host in ["wiki.lan.example.com", "nas", "blog.example.com"] {
        let r = over_plain(http, &get(host, "/a?b=1")).await;
        assert!(r.starts_with("HTTP/1.1 200"), "{host}: {r}");
        assert!(r.contains("GET /a?b=1\n"), "{r}");
        assert!(r.contains("x-forwarded-proto: http\n"), "{r}");
        assert!(r.contains("x-forwarded-port: 80\n"), "{r}");
    }
    assert!(
        closed_without_tls(s.proxy.https_addr().unwrap(), "wiki.lan.example.com").await,
        "a plain site has no HTTPS"
    );
}

#[tokio::test]
async fn other_https_names_are_sent_to_https_with_a_308() {
    let s = setup().await;
    let http = s.proxy.http_addr().unwrap();
    let https = s.proxy.https_addr().unwrap().port();
    for host in ["shop.example.com", "a.shop.example.com"] {
        let r = over_plain(http, &get(&format!("{host}:80"), "/cart?id=7")).await;
        assert!(r.starts_with("HTTP/1.1 308"), "{r}");
        let want = format!("location: https://{host}:{https}/cart?id=7\r\n");
        assert!(r.to_ascii_lowercase().contains(&want), "{r}");
    }
    assert_eq!(
        s.app.connections.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn a_name_whose_certificate_is_pending_or_failing_gets_the_pending_page() {
    let s = setup().await;
    let http = s.proxy.http_addr().unwrap();
    for state in [CertState::Pending, CertState::Failing] {
        s.certs
            .state
            .lock()
            .unwrap()
            .insert("shop.example.com".into(), state);
        let r = over_plain(http, &get("shop.example.com", "/")).await;
        assert!(r.starts_with("HTTP/1.1 503"), "{state:?}: {r}");
        assert!(r.contains("certificate is on its way"), "{r}");
        assert!(r.to_ascii_lowercase().contains("retry-after: 60"), "{r}");
    }
}

#[tokio::test]
async fn passthrough_tunnel_and_unknown_names_get_404_on_port_80() {
    let s = setup().await;
    let http = s.proxy.http_addr().unwrap();
    for host in [
        "files.example.com",
        "tunnel.example.com",
        "other.example.com",
    ] {
        let r = over_plain(http, &get(host, "/")).await;
        assert!(r.starts_with("HTTP/1.1 404"), "{host}: {r}");
    }
    let r = over_plain(http, "GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;
    assert!(r.starts_with("HTTP/1.1 400"), "{r}");
}

#[tokio::test]
async fn an_acme_http_challenge_is_answered_before_any_routing() {
    let s = setup().await;
    let http = s.proxy.http_addr().unwrap();
    s.certs
        .tokens
        .lock()
        .unwrap()
        .insert("tok_EN-1".into(), "tok_EN-1.thumb".into());
    for host in [
        "shop.example.com",
        "files.example.com",
        "unknown.example.com",
        "203.0.113.10",
    ] {
        let r = over_plain(http, &get(host, "/.well-known/acme-challenge/tok_EN-1")).await;
        assert!(r.starts_with("HTTP/1.1 200"), "{host}: {r}");
        assert!(r.ends_with("\r\n\r\ntok_EN-1.thumb"), "{r}");
        assert!(
            r.to_ascii_lowercase()
                .contains("content-type: application/octet-stream"),
            "{r}"
        );
    }
    let r = over_plain(
        http,
        &get("shop.example.com", "/.well-known/acme-challenge/other"),
    )
    .await;
    assert!(
        r.starts_with("HTTP/1.1 308"),
        "an unknown token is routed as usual: {r}"
    );
}
