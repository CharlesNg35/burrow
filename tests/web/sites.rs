use std::path::Path;

use burrow::config::settings::{ServerSettings, WebTls};
use burrow::wire::secret::Secret;
use tokio::io::AsyncWriteExt;

use crate::common::*;
use crate::kit::*;

pub fn with_files(s: &mut ServerSettings, service: &str, c: &Cert, dir: &Path) {
    let (cert, key) = (
        dir.join(format!("{service}.pem")),
        dir.join(format!("{service}.key")),
    );
    std::fs::write(&cert, &c.cert_pem).unwrap();
    std::fs::write(&key, &c.key_pem).unwrap();
    let site = s.web.as_mut().unwrap().sites.get_mut(service).unwrap();
    site.tls = WebTls::Files { cert, key };
}

pub async fn https_get(addr: std::net::SocketAddr, name: &str, c: &Cert, req: &str) -> String {
    let mut t = visit(addr, name, c).await.unwrap();
    t.write_all(req.as_bytes()).await.unwrap();
    t.flush().await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::io::AsyncReadExt::read_to_end(&mut t, &mut out),
    )
    .await;
    String::from_utf8_lossy(&out).into_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_site_with_its_own_certificate_files_is_served_over_https_and_redirected_from_http() {
    let dir = tempfile::tempdir().unwrap();
    let blog_cert = cert(&["blog.example.com"]);
    let app = http_app().await;
    let secret = Secret::generate();
    let mut s = web_server(
        &secret,
        &[("blog", site(&["home"], app), &["blog.example.com"])],
    );
    with_files(&mut s, "blog", &blog_cert, dir.path());
    let http = format!("127.0.0.1:{}", free_port()).parse().unwrap();
    s.web.as_mut().unwrap().http = Some(http);
    let t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    let web = t.server.web_addr.unwrap();
    let get = "GET /a HTTP/1.1\r\nHost: blog.example.com\r\nConnection: close\r\n\r\n";
    let r = https_get(web, "blog.example.com", &blog_cert, get).await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    assert!(r.contains("x-forwarded-proto: https\r\n"), "{r}");
    let other = "GET / HTTP/1.1\r\nHost: other.example.com\r\nConnection: close\r\n\r\n";
    let r = https_get(web, "blog.example.com", &blog_cert, other).await;
    assert!(r.starts_with("HTTP/1.1 421"), "{r}");
    let r = plain_get(http, "blog.example.com", "/a?b=1").await;
    assert!(r.starts_with("HTTP/1.1 308"), "{r}");
    let want = format!(
        "location: https://blog.example.com:{}/a?b=1\r\n",
        web.port()
    );
    assert!(r.to_ascii_lowercase().contains(&want), "{r}");
}
