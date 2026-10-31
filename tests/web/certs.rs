use std::path::Path;

use burrow::config::settings::{ServerSettings, WebTls, digest_of};
use burrow::metrics::{CertSourceStatus, CertStateStatus};
use burrow::wire::secret::Secret;
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;

use crate::common::*;
use crate::kit::*;

fn own_file(s: &mut ServerSettings, dir: &Path) {
    let web = s.web.as_mut().unwrap();
    web.sites.get_mut("blog").unwrap().tls = WebTls::Files {
        cert: dir.join("blog.pem"),
        key: dir.join("blog.key"),
    };
    web.file_digest = digest_of(&web.cert_files());
}

fn write(dir: &Path, c: &Cert) {
    std::fs::write(dir.join("blog.pem"), &c.cert_pem).unwrap();
    std::fs::write(dir.join("blog.key"), &c.key_pem).unwrap();
}

async fn shows(web: std::net::SocketAddr, c: &Cert) -> bool {
    let Ok(tcp) = TcpStream::connect(web).await else {
        return false;
    };
    let name = ServerName::try_from("blog.example.com").unwrap();
    trusting(c).connect(name, tcp).await.is_ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sites_own_file_is_served_shown_in_status_and_rewritten_in_place_on_reload() {
    let dir = tempfile::tempdir().unwrap();
    let (first, second) = (cert(&["blog.example.com"]), cert(&["blog.example.com"]));
    write(dir.path(), &first);
    let nas_cert = cert(&["nas.example.com"]);
    let (nas, _seen) = backend("nas", &nas_cert, false).await;
    let app = counting_backend().await;
    let secret = Secret::generate();
    let mut s = web_server(
        &secret,
        &[
            ("blog", site(&["home"], app), &["blog.example.com"]),
            ("nas", site(&["home"], nas), &["nas.example.com"]),
        ],
    );
    own_file(&mut s, dir.path());
    let t = tunnel_with(s.clone(), Vec::new(), |c| c, secret).await;
    let web = t.server.web_addr.unwrap();
    assert!(shows(web, &first).await, "the site's own file");
    let status = t.server.shared.status();
    let blog = status
        .certificates
        .iter()
        .find(|c| c.name == "blog.example.com")
        .unwrap();
    assert_eq!(
        blog.source,
        CertSourceStatus::OwnFile {
            path: dir.path().join("blog.pem").display().to_string()
        }
    );
    assert_eq!(blog.state, CertStateStatus::Ok);
    assert!(blog.expires.is_some());
    let nas = status
        .certificates
        .iter()
        .find(|c| c.name == "nas.example.com")
        .unwrap();
    assert_eq!(nas.source, CertSourceStatus::Passthrough);
    assert!(status.acme.is_none(), "no Let's Encrypt, no account");

    write(dir.path(), &second);
    let mut next = s.clone();
    own_file(&mut next, dir.path());
    assert_ne!(next.web, s.web, "the new bytes change the settings");
    t.server.reload_applied(next.clone()).await;
    assert!(shows(web, &second).await, "the rewritten file is served");
    assert!(!shows(web, &first).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_open_https_connection_keeps_working_after_its_certificate_is_swapped() {
    let dir = tempfile::tempdir().unwrap();
    let (first, second) = (cert(&["blog.example.com"]), cert(&["blog.example.com"]));
    write(dir.path(), &first);
    let secret = Secret::generate();
    let mut s = web_server(
        &secret,
        &[(
            "blog",
            site(&["home"], http_app().await),
            &["blog.example.com"],
        )],
    );
    own_file(&mut s, dir.path());
    let t = tunnel_with(s.clone(), Vec::new(), |c| c, secret).await;
    let web = t.server.web_addr.unwrap();
    let get = |path: &str| format!("GET {path} HTTP/1.1\r\nHost: blog.example.com\r\n\r\n");
    let mut kept = visit(web, "blog.example.com", &first).await.unwrap();
    let r = ask(&mut kept, get("/1").as_bytes()).await.unwrap();
    assert!(String::from_utf8_lossy(&r).contains("get /1 "), "{r:?}");
    write(dir.path(), &second);
    let mut next = s.clone();
    own_file(&mut next, dir.path());
    t.server.reload_applied(next.clone()).await;
    assert!(
        shows(web, &second).await,
        "new connections get the new file"
    );
    assert!(!shows(web, &first).await);
    let r = ask(&mut kept, get("/2").as_bytes()).await.unwrap();
    assert!(
        String::from_utf8_lossy(&r).contains("get /2 "),
        "the open connection keeps its session: {r:?}"
    );
}

async fn shows_name(web: std::net::SocketAddr, name: &str, c: &Cert) -> bool {
    let Ok(tcp) = TcpStream::connect(web).await else {
        return false;
    };
    let name = ServerName::try_from(name.to_string()).unwrap();
    trusting(c).connect(name, tcp).await.is_ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_websocket_tls_file_rewritten_at_its_path_is_served_on_the_tunnels_name_after_reload() {
    let (first, second) = (cert(&["localhost"]), cert(&["localhost"]));
    let secret = Secret::generate();
    let mut s = web_server(&secret, &[]);
    s.web.as_mut().unwrap().https = Some(format!("127.0.0.1:{}", free_port()).parse().unwrap());
    share_tunnel(&mut s, "localhost", &first);
    let srv = start_server(s.clone()).await;
    let web = srv.web_addr.unwrap();
    assert!(
        shows_name(web, "localhost", &first).await,
        "websocket.tls is served"
    );
    let mut next = s.clone();
    let tls = next.websocket.as_mut().unwrap().tls.as_mut().unwrap();
    std::fs::write(&tls.cert_path, &second.cert_pem).unwrap();
    std::fs::write(&tls.key_path, &second.key_pem).unwrap();
    tls.cert_pem = second.cert_pem.clone().into_bytes();
    tls.key_pem = zeroize::Zeroizing::new(second.key_pem.clone().into_bytes());
    assert_ne!(
        next.websocket, s.websocket,
        "the new bytes change the settings"
    );
    srv.reload(next.clone()).await;
    let shared = srv.shared.clone();
    wait_for(|| shared.settings_snapshot().websocket == next.websocket).await;
    let start = std::time::Instant::now();
    while !shows_name(web, "localhost", &second).await {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "the rewritten file is not served"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(!shows_name(web, "localhost", &first).await);
}
