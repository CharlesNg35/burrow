use std::time::Duration;

use burrow::client::State;
use burrow::wire::secret::Secret;
use burrow_config::Protocol;
use burrow_syntax::endpoint::Endpoint;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::common::*;
use crate::kit::*;

pub fn wss_client(
    url: &str,
    secret: Secret,
    ca: &Cert,
) -> burrow::config::settings::ClientSettings {
    let mut c = client_settings("127.0.0.1:1".parse().unwrap(), secret);
    c.server = url.into();
    c.endpoint = Endpoint::parse(url).unwrap();
    c.ca_pem = Some(ca.cert_pem.clone().into_bytes());
    c
}

async fn shared_server() -> (RunningServer, Secret, Cert, Cert, std::net::SocketAddr) {
    shared_server_with(|_| {}).await
}

async fn shared_server_with(
    edit: impl FnOnce(&mut burrow::config::settings::ServerSettings),
) -> (RunningServer, Secret, Cert, Cert, std::net::SocketAddr) {
    logs();
    let nas_cert = cert(&["nas.example.com"]);
    let (nas, _seen) = backend("nas", &nas_cert, false).await;
    let echo = echo_server().await;
    let secret = Secret::generate();
    let tunnel_cert = cert(&["localhost"]);
    let mut s = web_server(
        &secret,
        &[("nas", site(&["home"], nas), &["nas.example.com"])],
    );
    s.services.insert("db".into(), service(echo, Protocol::Tcp));
    s.web.as_mut().unwrap().https = Some(format!("127.0.0.1:{}", free_port()).parse().unwrap());
    share_tunnel(&mut s, "localhost", &tunnel_cert);
    edit(&mut s);
    let srv = start_server(s).await;
    assert_eq!(srv.ws_addr, None, "the tunnel has no listener of its own");
    let web = srv.web_addr.expect("web listener");
    (srv, secret, tunnel_cert, nas_cert, web)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_over_wss_shares_the_https_port_with_passthrough_sites() {
    let (srv, secret, tunnel_cert, nas_cert, web) = shared_server().await;
    let url = format!("wss://localhost:{}/burrow", web.port());
    let mut cli = start_client(wss_client(&url, secret, &tunnel_cert)).await;
    wait_state(&mut cli.state, State::Connected).await;
    let shared = srv.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 1).await;
    let data = pattern(2 << 20, 7);
    assert_eq!(roundtrip(srv.services["db"], &data).await, data);
    let mut a = visit(web, "nas.example.com", &nas_cert).await.unwrap();
    assert_eq!(ask(&mut a, b"side").await.unwrap(), b"nas:side");
    assert_eq!(srv.shared.session_names(), ["home"]);
    let status = srv.shared.status();
    let summary = status.web.expect("the status has the web summary");
    assert_eq!(summary.tunnel.unwrap().url, url);
    assert_eq!(summary.hosts[0].name, "nas.example.com");
    assert!(
        status
            .listening
            .iter()
            .any(|l| l.contains("(the web port)")),
        "{:?}",
        status.listening
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wrong_tunnel_path_is_refused_and_the_client_never_connects() {
    let (_srv, secret, tunnel_cert, _nas, web) = shared_server().await;
    let mut t = visit(web, "localhost", &tunnel_cert).await.unwrap();
    t.write_all(b"GET /wrong HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n")
        .await
        .unwrap();
    let mut head = vec![0u8; 64];
    let n = t.read(&mut head).await.unwrap();
    assert!(
        head[..n].starts_with(b"HTTP/1.1 404"),
        "{:?}",
        String::from_utf8_lossy(&head[..n])
    );
    let url = format!("wss://localhost:{}/other", web.port());
    let cli = start_client(wss_client(&url, secret, &tunnel_cert)).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_ne!(*cli.state.borrow(), State::Connected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overload_and_failed_tunnel_handshakes_on_the_web_port_are_counted() {
    let (srv, _secret, _tunnel_cert, _nas, web) =
        shared_server_with(|s| s.max_handshakes = 4).await;
    let before = srv.shared.status().handshakes_refused;
    let mut silent = Vec::new();
    for _ in 0..8 {
        silent.push(tokio::net::TcpStream::connect(web).await.unwrap());
    }
    let shared = srv.shared.clone();
    wait_for(|| shared.status().handshakes_refused >= before + 4).await;
    drop(silent);
    free_again(&srv, web).await;
    let after_flood = srv.shared.status().handshakes_refused;
    let mut bad = tokio::net::TcpStream::connect(web).await.unwrap();
    bad.write_all(&client_hello("localhost")).await.unwrap();
    bad.write_all(&[0x15, 0x03, 0x03, 0x00, 0x02, 0x02, 0x28])
        .await
        .unwrap();
    let mut buf = [0u8; 64];
    let answered = tokio::time::timeout(Duration::from_secs(5), bad.read(&mut buf)).await;
    assert!(
        matches!(answered, Ok(Ok(n)) if n > 0 && buf[0] == 0x16),
        "the server took part in the handshake, so this is a failed handshake, not an overload: {answered:?}"
    );
    wait_for(|| shared.status().handshakes_refused > after_flood).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        srv.shared.status().handshakes_refused,
        after_flood + 1,
        "the failed TLS handshake is counted once, and nothing else was refused"
    );
}

async fn free_again(srv: &RunningServer, web: std::net::SocketAddr) {
    let opened = || {
        let status = srv.shared.status();
        status
            .services
            .iter()
            .find(|s| s.name == "nas")
            .map_or(0, |s| s.total)
    };
    for _ in 0..100 {
        let before = opened();
        let mut probe = tokio::net::TcpStream::connect(web).await.unwrap();
        let _ = probe.write_all(&client_hello("nas.example.com")).await;
        for _ in 0..50 {
            if opened() > before {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    panic!("the pending slots never freed after the flood closed");
}

async fn reencrypting_relay(
    cert: &Cert,
    upstream: std::net::SocketAddr,
) -> (
    std::net::SocketAddr,
    tokio::sync::mpsc::UnboundedReceiver<std::io::Result<(u64, u64)>>,
) {
    let (ended, ends) = tokio::sync::mpsc::unbounded_channel();
    let acceptor = acceptor(cert);
    let connector = trusting(cert);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((down, _)) = l.accept().await {
            let (acceptor, connector, ended) = (acceptor.clone(), connector.clone(), ended.clone());
            tokio::spawn(async move {
                let Ok(mut down) = acceptor.accept(down).await else {
                    return;
                };
                let Ok(tcp) = tokio::net::TcpStream::connect(upstream).await else {
                    return;
                };
                let name =
                    tokio_rustls::rustls::pki_types::ServerName::try_from("localhost").unwrap();
                let Ok(mut up) = connector.connect(name, tcp).await else {
                    return;
                };
                let _ = ended.send(tokio::io::copy_bidirectional(&mut down, &mut up).await);
            });
        }
    });
    (addr, ends)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tunnel_channel_on_the_web_port_ends_with_tls_close_notify() {
    let (srv, secret, tunnel_cert, _nas, web) = shared_server().await;
    let (relay, mut ends) = reencrypting_relay(&tunnel_cert, web).await;
    let url = format!("wss://localhost:{}/burrow", relay.port());
    let mut cli = start_client(wss_client(&url, secret, &tunnel_cert)).await;
    wait_state(&mut cli.state, State::Connected).await;
    let data = pattern(8 << 20, 11);
    assert_eq!(roundtrip(srv.services["db"], &data).await, data);
    let end = tokio::time::timeout(Duration::from_secs(10), ends.recv())
        .await
        .expect("the data channel closes")
        .unwrap();
    assert!(
        end.is_ok(),
        "the data channel must end with a WebSocket close and TLS close_notify, not {end:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_web_block_without_sites_serves_the_tunnel_and_closes_other_names() {
    logs();
    let echo = echo_server().await;
    let secret = Secret::generate();
    let tunnel_cert = cert(&["localhost"]);
    let mut s = web_server(&secret, &[]);
    s.services.insert("db".into(), service(echo, Protocol::Tcp));
    s.web.as_mut().unwrap().https = Some(format!("127.0.0.1:{}", free_port()).parse().unwrap());
    share_tunnel(&mut s, "localhost", &tunnel_cert);
    let srv = start_server(s).await;
    let web = srv.web_addr.expect("the web port runs without sites");
    let url = format!("wss://localhost:{}/burrow", web.port());
    let mut cli = start_client(wss_client(&url, secret, &tunnel_cert)).await;
    wait_state(&mut cli.state, State::Connected).await;
    let shared = srv.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 1).await;
    let data = pattern(1 << 16, 8);
    assert_eq!(roundtrip(srv.services["db"], &data).await, data);
    assert!(closed(web, "other.example.com").await);
}
