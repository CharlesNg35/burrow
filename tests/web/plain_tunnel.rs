use std::time::Duration;

use burrow::client::State;
use burrow::config::settings::{ServerSettings, Shares, WebSocketSettings};
use burrow::wire::secret::Secret;
use burrow_config::Protocol;
use burrow_syntax::endpoint::Endpoint;

use crate::common::*;
use crate::kit::*;

fn tunnel_on_http(s: &mut ServerSettings, host: &str) {
    let http = s.web.as_ref().unwrap().http.unwrap();
    s.listen = None;
    s.websocket = Some(WebSocketSettings {
        listen: http,
        path: "/burrow".into(),
        tls: None,
        keepalive: Duration::from_secs(25),
        host: Some(host.into()),
        shares: Shares::Http,
    });
    s.web.as_mut().unwrap().tunnel = Some(host.into());
}

async fn connected_over_ws(s: ServerSettings, secret: Secret) -> (RunningServer, RunningClient) {
    let http = s.web.as_ref().unwrap().http.unwrap();
    let srv = start_server(s).await;
    assert_eq!(srv.ws_addr, None, "the tunnel has no listener of its own");
    let url = format!("ws://localhost:{}/burrow", http.port());
    let mut c = client_settings("127.0.0.1:1".parse().unwrap(), secret);
    c.server = url.clone();
    c.endpoint = Endpoint::parse(&url).unwrap();
    let mut cli = start_client(c).await;
    wait_state(&mut cli.state, State::Connected).await;
    let shared = srv.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 1).await;
    (srv, cli)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_over_plain_ws_on_its_own_name_shares_port_80_with_a_plain_site() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let mut s = plain_server(
        &secret,
        &[("blog", site(&["home"], http_app().await), &["127.0.0.1"])],
    );
    s.services.insert("db".into(), service(echo, Protocol::Tcp));
    tunnel_on_http(&mut s, "localhost");
    let http = s.web.as_ref().unwrap().http.unwrap();
    let (srv, _cli) = connected_over_ws(s, secret).await;
    let data = pattern(1 << 20, 5);
    assert_eq!(roundtrip(srv.services["db"], &data).await, data);
    let r = plain_get(http, "127.0.0.1", "/index").await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    let r = plain_get(http, "localhost", "/burrow").await;
    assert!(r.starts_with("HTTP/1.1 426"), "{r}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_over_plain_ws_takes_its_path_on_a_plain_sites_name() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let mut s = plain_server(
        &secret,
        &[("blog", site(&["home"], http_app().await), &["localhost"])],
    );
    s.services.insert("db".into(), service(echo, Protocol::Tcp));
    tunnel_on_http(&mut s, "localhost");
    let http = s.web.as_ref().unwrap().http.unwrap();
    let (srv, _cli) = connected_over_ws(s, secret).await;
    let data = pattern(1 << 16, 6);
    assert_eq!(roundtrip(srv.services["db"], &data).await, data);
    let r = plain_get(http, "localhost", "/index").await;
    assert!(
        r.starts_with("HTTP/1.1 200"),
        "the site keeps the rest: {r}"
    );
    assert!(r.contains("get /index http/1.1"), "{r}");
}
