use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use burrow::config::settings::{CertFile, LetsEncrypt, ServerSettings, WebTls, digest_of};
use burrow::wire::secret::Secret;
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;

use crate::common::*;
use crate::kit::*;

fn files(dir: &Path, stem: &str, c: &Cert) -> (PathBuf, PathBuf) {
    let (cert, key) = (
        dir.join(format!("{stem}.pem")),
        dir.join(format!("{stem}.key")),
    );
    std::fs::write(&cert, &c.cert_pem).unwrap();
    std::fs::write(&key, &c.key_pem).unwrap();
    (cert, key)
}

fn listed(hosts: &[&str], (cert, key): (PathBuf, PathBuf)) -> CertFile {
    CertFile {
        hosts: hosts.iter().map(|h| h.to_string()).collect(),
        cert,
        key,
    }
}

fn set_tls(s: &mut ServerSettings, site: &str, tls: WebTls) {
    s.web.as_mut().unwrap().sites.get_mut(site).unwrap().tls = tls;
}

fn seal(s: &mut ServerSettings) {
    let web = s.web.as_mut().unwrap();
    web.file_digest = digest_of(&web.cert_files());
}

async fn shows(web: SocketAddr, name: &str, c: &Cert) -> bool {
    let Ok(tcp) = TcpStream::connect(web).await else {
        return false;
    };
    let name = ServerName::try_from(name.to_string()).unwrap();
    trusting(c).connect(name, tcp).await.is_ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_name_gets_the_file_the_selection_order_gives_it_and_no_other() {
    let dir = tempfile::tempdir().unwrap();
    let own = cert(&["blog.example.com"]);
    let star = cert(&["*.example.com", "other.example.org"]);
    let exact = cert(&["shop.example.com"]);
    let app = http_app().await;
    let secret = Secret::generate();
    let mut s = web_server(
        &secret,
        &[
            ("blog", site(&["home"], app), &["blog.example.com"]),
            ("shop", site(&["home"], app), &["shop.example.com"]),
            ("apps", site(&["home"], app), &["*.example.com"]),
            ("other", site(&["home"], app), &["other.example.org"]),
        ],
    );
    let (oc, ok) = files(dir.path(), "own", &own);
    set_tls(&mut s, "blog", WebTls::Files { cert: oc, key: ok });
    for name in ["shop", "apps", "other"] {
        set_tls(&mut s, name, WebTls::Auto);
    }
    s.web.as_mut().unwrap().certificates = vec![
        listed(&["*.example.com"], files(dir.path(), "star", &star)),
        listed(&["shop.example.com"], files(dir.path(), "exact", &exact)),
    ];
    seal(&mut s);
    let srv = start_server(s).await;
    let web = srv.web_addr.unwrap();
    assert!(
        shows(web, "blog.example.com", &own).await,
        "the site's own file first"
    );
    assert!(!shows(web, "blog.example.com", &star).await);
    assert!(
        shows(web, "shop.example.com", &exact).await,
        "the exact entry before the wildcard one"
    );
    assert!(!shows(web, "shop.example.com", &star).await);
    assert!(
        shows(web, "x.example.com", &star).await,
        "the wildcard entry"
    );
    assert!(
        closed(web, "other.example.org").await,
        "a shared file is never used for a name its entry does not list"
    );
    assert!(closed(web, "a.b.example.com").await, "one label only");
    assert!(
        closed(web, "unknown.test").await,
        "an unknown name is closed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_name_waiting_for_its_certificate_gets_the_pending_page_then_a_308_once_one_exists() {
    let dir = tempfile::tempdir().unwrap();
    let blog = cert(&["blog.example.com"]);
    let secret = Secret::generate();
    let mut s = web_server(
        &secret,
        &[(
            "blog",
            site(&["home"], http_app().await),
            &["blog.example.com"],
        )],
    );
    set_tls(&mut s, "blog", WebTls::Auto);
    let http: SocketAddr = format!("127.0.0.1:{}", free_port()).parse().unwrap();
    let web = s.web.as_mut().unwrap();
    web.http = Some(http);
    web.letsencrypt = Some(LetsEncrypt {
        email: "ops@example.com".into(),
        directory: "https://127.0.0.1:1/directory".into(),
        ca: None,
        profile: None,
        state_dir: Some(dir.path().join("state")),
        dns: Vec::new(),
        secret_files: Vec::new(),
    });
    let srv = start_server(s.clone()).await;
    let r = plain_get(http, "blog.example.com", "/a?b=1").await;
    assert!(r.starts_with("HTTP/1.1 503"), "{r}");
    assert!(r.contains("certificate is on its way"), "{r}");
    assert!(r.to_ascii_lowercase().contains("retry-after: 60"), "{r}");
    let https = srv.web_addr.unwrap();
    assert!(
        closed(https, "blog.example.com").await,
        "no certificate yet"
    );

    let mut next = s.clone();
    next.web.as_mut().unwrap().certificates = vec![listed(
        &["blog.example.com"],
        files(dir.path(), "blog", &blog),
    )];
    seal(&mut next);
    srv.reload(next).await;
    let start = Instant::now();
    let r = loop {
        let r = plain_get(http, "blog.example.com", "/a?b=1").await;
        if !r.starts_with("HTTP/1.1 503") {
            break r;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "{r}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert!(r.starts_with("HTTP/1.1 308"), "{r}");
    let want = format!(
        "location: https://blog.example.com:{}/a?b=1\r\n",
        https.port()
    );
    assert!(r.to_ascii_lowercase().contains(&want), "{r}");
    assert!(shows(https, "blog.example.com", &blog).await);
}
