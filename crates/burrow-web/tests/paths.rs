mod common;

use common::*;

async fn blog() -> (burrow_web::Proxy<Fake>, HttpBackend) {
    let app = http_backend().await;
    let (up, _) = Fake::new(4);
    up.serve("blog", app.addr);
    let proxy = start(
        |s| s.sites = vec![https_site("blog", &["blog.example.com"])],
        up.clone(),
    )
    .await;
    (proxy, app)
}

async fn get(proxy: &burrow_web::Proxy<Fake>, target: &str) -> String {
    over_tls(
        proxy.https_addr().unwrap(),
        "blog.example.com",
        &format!("GET {target} HTTP/1.1\r\nHost: blog.example.com\r\nConnection: close\r\n\r\n"),
    )
    .await
}

#[tokio::test]
async fn the_app_receives_the_normalized_path_and_the_query_unchanged() {
    let (proxy, _app) = blog().await;
    for (sent, seen) in [
        ("//admin", "GET /admin"),
        ("/a//b///c?x=%2f//", "GET /a/b/c?x=%2f//"),
        ("/%61pi/x", "GET /api/x"),
        ("/%2fx", "GET /%2Fx"),
        (
            "/api/v4/projects/group%2Fproject",
            "GET /api/v4/projects/group%2Fproject",
        ),
        ("/caf\u{e9}", "GET /caf%C3%A9"),
        ("https://blog.example.com//x?y", "GET /x?y"),
    ] {
        let r = get(&proxy, sent).await;
        assert!(r.starts_with("HTTP/1.1 200"), "{sent}: {r}");
        assert!(r.contains(&format!("\n{seen}\n")), "{sent}: {r}");
    }
}

#[tokio::test]
async fn paths_with_dot_segments_or_bad_escapes_are_refused_before_the_app() {
    let (proxy, app) = blog().await;
    for sent in [
        "/public%2F..%2Fadmin",
        "/a%5C..%5Cb",
        "/%2e%2e/etc/passwd",
        "/a/%2E/b",
        "/a/../b",
        "/a%00b",
        "/a%zz",
    ] {
        let r = get(&proxy, sent).await;
        assert!(r.starts_with("HTTP/1.1 400"), "{sent}: {r}");
    }
    assert_eq!(app.connections.load(std::sync::atomic::Ordering::SeqCst), 0);
}
