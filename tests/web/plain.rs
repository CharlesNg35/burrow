use burrow::wire::secret::Secret;

use crate::common::*;
use crate::kit::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_server_with_only_web_http_serves_a_plain_site_through_its_client() {
    let app = http_app().await;
    let secret = Secret::generate();
    let s = plain_server(
        &secret,
        &[(
            "wiki",
            site(&["home"], app),
            &["wiki.lan.example.com", "nas"],
        )],
    );
    let http = s.web.as_ref().unwrap().http.unwrap();
    let t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    assert_eq!(t.server.web_addr, None, "no HTTPS port without web.https");
    for host in ["wiki.lan.example.com", "nas"] {
        let r = plain_get(http, host, "/page?x=1").await;
        assert!(r.starts_with("HTTP/1.1 200"), "{host}: {r}");
        assert!(r.contains("get /page?x=1 http/1.1"), "{r}");
        assert!(r.contains("x-forwarded-proto: http\r\n"), "{r}");
        assert!(r.contains(&format!("host: {host}\r\n")), "{r}");
    }
    let r = plain_get(http, "other.example.com", "/").await;
    assert!(r.starts_with("HTTP/1.1 404"), "{r}");
    let status = t.server.shared.status();
    assert!(
        status
            .listening
            .iter()
            .any(|l| l == &format!("websites on http {http}")),
        "{:?}",
        status.listening
    );
}
