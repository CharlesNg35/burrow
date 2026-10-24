mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use burrow::client::State;
use burrow::config::settings::{ClientSettings, ServerSettings, TlsMaterial, WebSocketSettings};
use burrow::wire::secret::Secret;
use burrow_config::Protocol;
use burrow_syntax::endpoint::Endpoint;
use common::*;
use tokio::net::{TcpListener, TcpStream};

struct Cert {
    cert_pem: String,
    key_pem: String,
}

fn self_signed() -> Cert {
    let key = rcgen::KeyPair::generate().unwrap();
    let params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    let cert = params.self_signed(&key).unwrap();
    Cert {
        cert_pem: cert.pem(),
        key_pem: key.serialize_pem(),
    }
}

fn ws_server(secret: &Secret, path: &str, tls: Option<&Cert>) -> ServerSettings {
    let mut s = server_settings(vec![secret.clone()]);
    s.listen = None;
    s.websocket = Some(WebSocketSettings {
        listen: "127.0.0.1:0".parse().unwrap(),
        path: path.into(),
        tls: tls.map(|c| TlsMaterial {
            cert_path: "cert.pem".into(),
            key_path: "key.pem".into(),
            cert_pem: c.cert_pem.clone().into_bytes(),
            key_pem: zeroize::Zeroizing::new(c.key_pem.clone().into_bytes()),
        }),
        keepalive: Duration::from_secs(25),
    });
    s
}

fn ws_client(url: &str, secret: Secret, ca: Option<&Cert>) -> ClientSettings {
    let mut c = client_settings("127.0.0.1:1".parse().unwrap(), secret);
    c.server = url.into();
    c.endpoint = Endpoint::parse(url).unwrap();
    c.ca_pem = ca.map(|c| c.cert_pem.clone().into_bytes());
    c
}

async fn connected(srv: &RunningServer, client: ClientSettings) -> RunningClient {
    logs();
    let mut cli = start_client(client).await;
    wait_state(&mut cli.state, State::Connected).await;
    let shared = srv.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 1).await;
    cli
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn websocket_only_server_forwards_tcp_and_udp() {
    let echo = echo_server().await;
    let uecho = udp_echo_server().await;
    let secret = Secret::generate();
    let mut s = ws_server(&secret, "/burrow", None);
    s.services
        .insert("web".into(), service(echo, Protocol::Tcp));
    s.services
        .insert("dns".into(), service(uecho, Protocol::Udp));
    let srv = start_server(s).await;
    let ws = srv.ws_addr.expect("websocket listener");
    let _cli = connected(&srv, ws_client(&format!("ws://{ws}/burrow"), secret, None)).await;

    let data = pattern(16 << 20, 5);
    assert_eq!(roundtrip(srv.services["web"], &data).await, data);
    let tasks: Vec<_> = (0..100)
        .map(|i| {
            let addr = srv.services["web"];
            tokio::spawn(async move {
                let d = pattern(1000 + i * 13, i as u64);
                assert_eq!(roundtrip(addr, &d).await, d);
            })
        })
        .collect();
    for t in tasks {
        t.await.unwrap();
    }

    let sock = udp_bind().await;
    sock.connect(srv.services["dns"]).await.unwrap();
    let mut buf = [0u8; 64];
    let mut ok = false;
    for _ in 0..50 {
        sock.send(b"over ws").await.unwrap();
        if let Ok(Ok(n)) =
            tokio::time::timeout(Duration::from_millis(100), sock.recv(&mut buf)).await
        {
            ok = &buf[..n] == b"over ws";
            break;
        }
    }
    assert!(ok, "udp over websocket");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wss_with_own_certificate() {
    let echo = echo_server().await;
    let cert = self_signed();
    let secret = Secret::generate();
    let mut s = ws_server(&secret, "/t", Some(&cert));
    s.services
        .insert("web".into(), service(echo, Protocol::Tcp));
    let srv = start_server(s).await;
    let port = srv.ws_addr.unwrap().port();
    let _cli = connected(
        &srv,
        ws_client(&format!("wss://localhost:{port}/t"), secret, Some(&cert)),
    )
    .await;
    let data = pattern(4 << 20, 8);
    assert_eq!(roundtrip(srv.services["web"], &data).await, data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wss_without_trusting_the_certificate_fails() {
    let cert = self_signed();
    let secret = Secret::generate();
    let srv = start_server(ws_server(&secret, "/t", Some(&cert))).await;
    let port = srv.ws_addr.unwrap().port();
    let cli = start_client(ws_client(
        &format!("wss://localhost:{port}/t"),
        secret,
        None,
    ))
    .await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(*cli.state.borrow(), State::Connecting);
    assert!(srv.shared.session_names().is_empty());
}

async fn tls_terminating_proxy(
    cert: &Cert,
    upstream: SocketAddr,
) -> (
    SocketAddr,
    tokio::sync::mpsc::UnboundedReceiver<std::io::Result<(u64, u64)>>,
) {
    let (ended, ends) = tokio::sync::mpsc::unbounded_channel();
    let acceptor =
        burrow::wire::transport::tls::acceptor(cert.cert_pem.as_bytes(), cert.key_pem.as_bytes())
            .unwrap();
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((down, _)) = l.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            let ended = ended.clone();
            tokio::spawn(async move {
                let Ok(mut down) = acceptor.accept(down).await else {
                    return;
                };
                let Ok(mut up) = TcpStream::connect(upstream).await else {
                    return;
                };
                let _ = ended.send(tokio::io::copy_bidirectional(&mut down, &mut up).await);
            });
        }
    });
    (addr, ends)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn behind_a_tls_terminating_proxy() {
    let echo = echo_server().await;
    let cert = self_signed();
    let secret = Secret::generate();
    let mut s = ws_server(&secret, "/burrow", None);
    s.services
        .insert("web".into(), service(echo, Protocol::Tcp));
    let srv = start_server(s).await;
    let (proxy, mut ends) = tls_terminating_proxy(&cert, srv.ws_addr.unwrap()).await;
    let _cli = connected(
        &srv,
        ws_client(
            &format!("wss://localhost:{}/burrow", proxy.port()),
            secret,
            Some(&cert),
        ),
    )
    .await;
    let data = pattern(8 << 20, 11);
    assert_eq!(roundtrip(srv.services["web"], &data).await, data);
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
async fn wrong_path_is_refused_and_client_keeps_retrying() {
    let secret = Secret::generate();
    let srv = start_server(ws_server(&secret, "/burrow", None)).await;
    let ws = srv.ws_addr.unwrap();
    let cli = start_client(ws_client(&format!("ws://{ws}/nope"), secret.clone(), None)).await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(*cli.state.borrow(), State::Connecting);
    assert!(!cli.task.is_finished());

    let mut raw = TcpStream::connect(ws).await.unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    raw.write_all(b"GET /other HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .unwrap();
    let mut reply = String::new();
    raw.read_to_string(&mut reply).await.unwrap();
    assert!(reply.starts_with("HTTP/1.1 404"), "{reply}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reload_changes_the_websocket_path() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let mut s = ws_server(&secret, "/one", None);
    s.services
        .insert("web".into(), service(echo, Protocol::Tcp));
    let srv = start_server(s.clone()).await;
    let ws = srv.ws_addr.unwrap();
    let mut next = s.clone();
    next.websocket.as_mut().unwrap().listen = ws;
    next.websocket.as_mut().unwrap().path = "/two".into();
    next.services.get_mut("web").unwrap().bind = Some(srv.services["web"]);
    srv.reload(next).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let _cli = connected(&srv, ws_client(&format!("ws://{ws}/two"), secret, None)).await;
    assert_eq!(roundtrip(srv.services["web"], b"moved").await, b"moved");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plaintext_setting_still_encrypts_over_websocket() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let mut s = ws_server(&secret, "/burrow", None);
    s.insecure_plaintext = true;
    s.services
        .insert("web".into(), service(echo, Protocol::Tcp));
    let srv = start_server(s).await;
    let ws = srv.ws_addr.unwrap();
    let mut c = ws_client(&format!("ws://{ws}/burrow"), secret, None);
    c.insecure_plaintext = true;
    let _cli = connected(&srv, c).await;
    let data = pattern(2 << 20, 3);
    assert_eq!(roundtrip(srv.services["web"], &data).await, data);
    let _ = Arc::new(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn websocket_listener_only_runs_when_configured() {
    let secret = Secret::generate();
    let tcp_only = burrow::server::Server::bind(server_settings(vec![secret.clone()]))
        .await
        .unwrap();
    assert!(
        tcp_only.websocket_addr().is_none(),
        "no websocket listener without a websocket section"
    );
    assert!(tcp_only.local_addr().is_some());
    let ws_only = burrow::server::Server::bind(ws_server(&secret, "/burrow", None))
        .await
        .unwrap();
    assert!(
        ws_only.local_addr().is_none(),
        "no TCP control port when only websocket is configured"
    );
    assert!(ws_only.websocket_addr().is_some());
}

async fn idle_killing_proxy(
    upstream: SocketAddr,
    idle: Duration,
) -> (SocketAddr, Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let opened = Arc::new(AtomicUsize::new(0));
    let count = opened.clone();
    tokio::spawn(async move {
        loop {
            let Ok((down, _)) = l.accept().await else {
                return;
            };
            count.fetch_add(1, Ordering::Relaxed);
            tokio::spawn(async move {
                let Ok(up) = TcpStream::connect(upstream).await else {
                    return;
                };
                let (mut dr, mut dw) = down.into_split();
                let (mut ur, mut uw) = up.into_split();
                let last = Arc::new(std::sync::Mutex::new(std::time::Instant::now()));
                let (l1, l2) = (last.clone(), last.clone());
                let a = async move {
                    let mut b = vec![0u8; 65536];
                    while let Ok(n) = dr.read(&mut b).await {
                        if n == 0 || uw.write_all(&b[..n]).await.is_err() {
                            break;
                        }
                        *l1.lock().unwrap() = std::time::Instant::now();
                    }
                };
                let c = async move {
                    let mut b = vec![0u8; 65536];
                    while let Ok(n) = ur.read(&mut b).await {
                        if n == 0 || dw.write_all(&b[..n]).await.is_err() {
                            break;
                        }
                        *l2.lock().unwrap() = std::time::Instant::now();
                    }
                };
                let watchdog = async move {
                    loop {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        if last.lock().unwrap().elapsed() > idle {
                            break;
                        }
                    }
                };
                tokio::select! { _ = a => {}, _ = c => {}, _ = watchdog => {} }
            });
        }
    });
    (addr, opened)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keepalive_keeps_pooled_channels_alive_through_an_idle_timeout_proxy() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let mut s = ws_server(&secret, "/burrow", None);
    s.heartbeat_interval = Duration::from_millis(400);
    s.websocket.as_mut().unwrap().keepalive = Duration::from_millis(400);
    s.services
        .insert("web".into(), service(echo, Protocol::Tcp));
    let srv = start_server(s).await;
    let (proxy, opened) =
        idle_killing_proxy(srv.ws_addr.unwrap(), Duration::from_millis(1500)).await;
    let _cli = connected(
        &srv,
        ws_client(&format!("ws://{proxy}/burrow"), secret, None),
    )
    .await;
    let shared = srv.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 2).await;
    let before = opened.load(std::sync::atomic::Ordering::Relaxed);
    tokio::time::sleep(Duration::from_secs(5)).await;
    let after = opened.load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(
        after, before,
        "no reconnects: keepalives kept every channel alive past the 1.5 s idle timeout"
    );
    assert_eq!(
        roundtrip(srv.services["web"], b"still warm").await,
        b"still warm"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn path_change_swaps_the_route_and_sends_old_path_clients_away() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port_addr = free.local_addr().unwrap();
    drop(free);
    let mut s = ws_server(&secret, "/one", None);
    for name in ["office", "lab"] {
        s.clients.insert(
            name.into(),
            burrow::config::settings::ClientAuth {
                secrets: vec![secret.clone()],
                max_connections: None,
            },
        );
    }
    s.websocket.as_mut().unwrap().listen = port_addr;
    s.services
        .insert("web".into(), service(echo, Protocol::Tcp));
    let srv = start_server(s.clone()).await;
    let mut first = connected(
        &srv,
        ws_client(&format!("ws://{port_addr}/one"), secret.clone(), None),
    )
    .await;

    let mut next = s.clone();
    next.websocket.as_mut().unwrap().path = "/two".into();
    next.services.get_mut("web").unwrap().bind = Some(srv.services["web"]);
    srv.reload(next).await;
    let left = tokio::time::timeout(
        Duration::from_secs(5),
        first.state.wait_for(|s| *s != State::Connected),
    )
    .await
    .is_ok_and(|r| r.is_ok());
    assert!(left, "a client on the old path is sent away");
    let named = |name: &str, path: &str| {
        let mut c = ws_client(&format!("ws://{port_addr}{path}"), secret.clone(), None);
        c.name = name.into();
        c
    };
    let mut second = start_client(named("home", "/two")).await;
    wait_state(&mut second.state, State::Connected).await;
    let shared = srv.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 1).await;
    assert_eq!(
        roundtrip(srv.services["web"], b"same socket").await,
        b"same socket"
    );
    let old_path = start_client(named("lab", "/one")).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(
        *old_path.state.borrow(),
        State::Connecting,
        "old path is refused after the reload"
    );
}

fn self_signed_ca_flagged() -> Cert {
    let key = rcgen::KeyPair::generate().unwrap();
    let mut params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let cert = params.self_signed(&key).unwrap();
    Cert {
        cert_pem: cert.pem(),
        key_pem: key.serialize_pem(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pinned_self_signed_certificate_marked_as_ca_is_accepted() {
    let echo = echo_server().await;
    let cert = self_signed_ca_flagged();
    let secret = Secret::generate();
    let mut s = ws_server(&secret, "/t", Some(&cert));
    s.services
        .insert("web".into(), service(echo, Protocol::Tcp));
    let srv = start_server(s).await;
    let port = srv.ws_addr.unwrap().port();
    let _cli = connected(
        &srv,
        ws_client(&format!("wss://localhost:{port}/t"), secret, Some(&cert)),
    )
    .await;
    assert_eq!(roundtrip(srv.services["web"], b"pinned").await, b"pinned");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pinning_a_different_certificate_is_refused() {
    let cert = self_signed_ca_flagged();
    let other = self_signed_ca_flagged();
    let secret = Secret::generate();
    let srv = start_server(ws_server(&secret, "/t", Some(&cert))).await;
    let port = srv.ws_addr.unwrap().port();
    let cli = start_client(ws_client(
        &format!("wss://localhost:{port}/t"),
        secret,
        Some(&other),
    ))
    .await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(*cli.state.borrow(), State::Connecting);
    assert!(srv.shared.session_names().is_empty());
}

fn dialer_for(url: &str, ca: Option<&Cert>) -> burrow::dialer::Dialer {
    let mut v = visitor_settings("127.0.0.1:1".parse().unwrap(), Secret::generate(), vec![]);
    v.server = url.into();
    v.endpoint = Endpoint::parse(url).unwrap();
    v.ca_pem = ca.map(|c| c.cert_pem.clone().into_bytes());
    burrow::dialer::Dialer::visitor(&v)
}

async fn dial(d: &burrow::dialer::Dialer) -> burrow::dialer::DialError {
    let hello = burrow::wire::protocol::Hello::Visit { version: 1 };
    match d
        .open(burrow::wire::protocol::ChannelKind::Visit, hello)
        .await
    {
        Ok(_) => panic!("the dial should fail"),
        Err(e) => e,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_untrusted_certificate_fails_at_once_instead_of_being_retried() {
    use burrow::dialer::DialError;
    let cert = self_signed();
    let srv = start_server(ws_server(&Secret::generate(), "/t", Some(&cert))).await;
    let port = srv.ws_addr.unwrap().port();
    let e = dial(&dialer_for(&format!("wss://localhost:{port}/t"), None)).await;
    assert!(matches!(e, DialError::Unreachable(_)), "{e:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_proxy_that_cannot_reach_a_busy_server_is_retried_but_a_wrong_path_is_not() {
    use burrow::dialer::DialError;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for (code, retried) in [(502, true), (503, true), (504, true), (404, false)] {
        let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = proxy.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut c, _)) = proxy.accept().await {
                let mut head = [0u8; 4096];
                let _ = c.read(&mut head).await;
                let _ = c
                    .write_all(format!("HTTP/1.1 {code} X\r\nContent-Length: 0\r\n\r\n").as_bytes())
                    .await;
            }
        });
        let e = dial(&dialer_for(&format!("ws://{addr}/burrow"), None)).await;
        let lost = matches!(e, DialError::Lost(_));
        assert_eq!(lost, retried, "HTTP {code}: {e:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_move_to_a_busy_port_keeps_the_listener_and_retries_on_the_next_reload() {
    let secret = Secret::generate();
    let s = ws_server(&secret, "/burrow", None);
    let srv = start_server(s.clone()).await;
    let ws = srv.ws_addr.unwrap();
    let mut first = s.clone();
    first.websocket.as_mut().unwrap().listen = ws;
    let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let elsewhere = busy.local_addr().unwrap();
    let mut moved = first.clone();
    moved.websocket.as_mut().unwrap().listen = elsewhere;
    srv.reload(moved.clone()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let kept = connected(
        &srv,
        ws_client(&format!("ws://{ws}/burrow"), secret.clone(), None),
    )
    .await;
    kept.stop.send(true).unwrap();
    kept.task.await.unwrap();
    drop(busy);
    srv.reload(moved).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _moved = connected(
        &srv,
        ws_client(&format!("ws://{elsewhere}/burrow"), secret, None),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn swapping_the_websocket_port_with_a_service_port_ends_with_both_listening() {
    logs();
    let secret = Secret::generate();
    let mut s = ws_server(&secret, "/burrow", None);
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let web_port = free.local_addr().unwrap();
    drop(free);
    let mut web = service("127.0.0.1:1".parse().unwrap(), Protocol::Tcp);
    web.bind = Some(web_port);
    s.services.insert("web".into(), web);
    let srv = start_server(s.clone()).await;
    let ws_port = srv.ws_addr.unwrap();
    let mut swapped = s.clone();
    swapped.websocket.as_mut().unwrap().listen = web_port;
    swapped.services.get_mut("web").unwrap().bind = Some(ws_port);
    srv.reload(swapped).await;
    let mut both = false;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if TcpStream::connect(ws_port).await.is_ok() && TcpStream::connect(web_port).await.is_ok() {
            both = true;
            break;
        }
    }
    assert!(both, "after the swap one of the two ports stayed closed");
    let _cli = connected(
        &srv,
        ws_client(&format!("ws://{web_port}/burrow"), secret, None),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_service_can_take_the_port_of_a_websocket_removed_in_the_same_reload() {
    if ws_mode() {
        return;
    }
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    logs();
    let secret = Secret::generate();
    let mut s = server_settings(vec![secret.clone()]);
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = free.local_addr().unwrap();
    drop(free);
    s.websocket = ws_server(&secret, "/burrow", None).websocket;
    s.websocket.as_mut().unwrap().listen = port;
    let srv = start_server(s.clone()).await;
    let mut next = s.clone();
    next.websocket = None;
    let mut web = service("127.0.0.1:1".parse().unwrap(), Protocol::Tcp);
    web.bind = Some(port);
    next.services.insert("web".into(), web);
    srv.reload(next).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let mut conn = TcpStream::connect(port)
        .await
        .expect("the service listens on the freed port");
    let _ = conn
        .write_all(b"GET /burrow HTTP/1.1\r\nHost: x\r\n\r\n")
        .await;
    let mut got = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(3), conn.read_to_end(&mut got)).await;
    assert!(!got.starts_with(b"HTTP/"), "the websocket still answers");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tcp_client_at_the_websocket_port_is_told_to_use_a_websocket_url() {
    if ws_mode() {
        return;
    }
    use burrow::dialer::{DialError, Dialer};
    use burrow::wire::protocol::{ChannelKind, Hello};
    let secret = Secret::generate();
    let srv = start_server(ws_server(&secret, "/burrow", None)).await;
    let ws = srv.ws_addr.unwrap();
    let dialer = Dialer::visitor(&visitor_settings(ws, secret, vec![]));
    let got = dialer
        .open(ChannelKind::Visit, Hello::Visit { version: 1 })
        .await;
    let Err(DialError::Lost(why)) = got else {
        panic!("expected a lost dial, got {:?}", got.err())
    };
    assert!(why.contains("ws://"), "{why}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn clients_of_a_websocket_path_that_changed_are_sent_to_reconnect() {
    let secret = Secret::generate();
    let s = ws_server(&secret, "/one", None);
    let srv = start_server(s.clone()).await;
    let ws = srv.ws_addr.unwrap();
    let mut cli = connected(&srv, ws_client(&format!("ws://{ws}/one"), secret, None)).await;
    let mut next = s.clone();
    next.websocket.as_mut().unwrap().listen = ws;
    next.websocket.as_mut().unwrap().path = "/two".into();
    srv.reload(next).await;
    let left = tokio::time::timeout(
        Duration::from_secs(5),
        cli.state.wait_for(|s| *s != State::Connected),
    )
    .await
    .is_ok_and(|r| r.is_ok());
    assert!(left, "the client stayed connected to a path that is gone");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wss_client_with_a_wrong_secret_is_reported_as_rejected() {
    logs();
    let cert = self_signed();
    let secret = Secret::generate();
    let srv = start_server(ws_server(&secret, "/t", Some(&cert))).await;
    let port = srv.ws_addr.unwrap().port();
    let mut cli = start_client(ws_client(
        &format!("wss://localhost:{port}/t"),
        Secret::generate(),
        Some(&cert),
    ))
    .await;
    let rejected = tokio::time::timeout(
        Duration::from_secs(10),
        cli.state.wait_for(|s| *s == State::Rejected),
    )
    .await
    .is_ok_and(|r| r.is_ok());
    assert!(rejected, "a wrong secret over wss is not reported");
}
