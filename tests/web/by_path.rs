use burrow::client::State;
use burrow::wire::secret::Secret;
use burrow_config::Protocol;

use crate::common::*;
use crate::kit::*;
use crate::sites::{https_get, with_files};
use crate::tunnel::wss_client;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_tunnel_by_path_shares_a_terminated_sites_name_and_certificate() {
    logs();
    let dir = tempfile::tempdir().unwrap();
    let site_cert = cert(&["localhost"]);
    let app = http_app().await;
    let echo = echo_server().await;
    let secret = Secret::generate();
    let mut s = web_server(&secret, &[("blog", site(&["home"], app), &["localhost"])]);
    with_files(&mut s, "blog", &site_cert, dir.path());
    s.services.insert("db".into(), service(echo, Protocol::Tcp));
    s.web.as_mut().unwrap().https = Some(format!("127.0.0.1:{}", free_port()).parse().unwrap());
    share_tunnel(&mut s, "localhost", &site_cert);
    s.websocket.as_mut().unwrap().tls = None;
    let srv = start_server(s).await;
    let web = srv.web_addr.unwrap();
    let url = format!("wss://localhost:{}/burrow", web.port());
    let mut cli = start_client(wss_client(&url, secret, &site_cert)).await;
    wait_state(&mut cli.state, State::Connected).await;
    let shared = srv.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 1).await;
    let data = pattern(1 << 20, 3);
    assert_eq!(roundtrip(srv.services["db"], &data).await, data);
    let page = "GET /index HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    let r = https_get(web, "localhost", &site_cert, page).await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    assert!(
        r.contains("get /index http/1.1"),
        "the site keeps the rest of its name: {r}"
    );
    let browser = "GET /burrow HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    let r = https_get(web, "localhost", &site_cert, browser).await;
    assert!(r.starts_with("HTTP/1.1 426"), "{r}");
}
