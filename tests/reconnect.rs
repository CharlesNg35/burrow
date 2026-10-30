mod common;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use burrow::client::{Client, RunEnd, State};
use burrow::wire::secret::Secret;
use burrow::wire::transport::resolve::{Resolved, Resolver, resolve};
use burrow_config::Protocol;
use common::*;

struct MovableDns {
    name: String,
    addr: Mutex<SocketAddr>,
}

impl Resolver for MovableDns {
    fn resolve<'a>(&'a self, host_port: &'a str) -> Resolved<'a> {
        Box::pin(async move {
            if host_port == self.name {
                Ok(vec![*self.addr.lock().unwrap()])
            } else {
                resolve(host_port).await
            }
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_follows_the_server_to_a_new_address() {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    settings
        .services
        .insert("web".into(), service(echo, Protocol::Tcp));
    let old = start_server(settings.clone()).await;

    let dns = Arc::new(MovableDns {
        name: "tunnel.example.test:7835".into(),
        addr: Mutex::new(old.addr),
    });
    let mut c = client_settings(old.addr, secret);
    c.server = if ws_mode() {
        "ws://tunnel.example.test:7835/burrow".into()
    } else {
        "tunnel.example.test:7835".into()
    };
    c.endpoint = burrow_syntax::endpoint::Endpoint::parse(&c.server).unwrap();
    let client = Client::new(c).with_resolver(dns.clone());
    let mut cli = start_client_with(client).await;
    wait_state(&mut cli.state, State::Connected).await;
    let shared = old.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 1).await;
    assert_eq!(
        roundtrip(old.services["web"], b"old home").await,
        b"old home"
    );

    old.stop.send(true).unwrap();
    old.task.await.unwrap();
    wait_state(&mut cli.state, State::Connecting).await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let mut moved = settings.clone();
    let elsewhere = SocketAddr::new(other_loopback(), 0);
    moved.listen = Some(elsewhere);
    for svc in moved.services.values_mut() {
        svc.bind = Some(elsewhere);
    }
    let new = start_server(moved).await;
    *dns.addr.lock().unwrap() = new.addr;

    wait_state(&mut cli.state, State::Connected).await;
    let shared = new.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 1).await;
    assert_eq!(
        roundtrip(new.services["web"], b"new home").await,
        b"new home"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retries_forever_by_default() {
    let addr = free_addr();
    let mut c = client_settings(addr, Secret::generate());
    c.reconnect_max = Duration::from_millis(50);
    let cli = start_client(c).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(
        !cli.task.is_finished(),
        "client keeps retrying while the server is down"
    );
    cli.stop.send(true).unwrap();
    assert_eq!(cli.task.await.unwrap(), RunEnd::Stopped);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gives_up_only_when_configured() {
    let addr = free_addr();
    let mut c = client_settings(addr, Secret::generate());
    c.reconnect_max = Duration::from_millis(100);
    c.give_up_after = Some(Duration::from_secs(1));
    let cli = start_client(c).await;
    let end = tokio::time::timeout(Duration::from_secs(5), cli.task)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(&end, RunEnd::GaveUp(d, why) if *d >= Duration::from_secs(1) && why.contains(&addr.to_string())),
        "{end:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exits_on_rejection_only_when_configured() {
    let srv = start_server(server_settings(vec![Secret::generate()])).await;
    let mut c = client_settings(srv.addr, Secret::generate());
    c.exit_on_rejected = true;
    let cli = start_client(c).await;
    let end = tokio::time::timeout(Duration::from_secs(5), cli.task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(end, RunEnd::Rejected);

    let retry = start_client(client_settings(srv.addr, Secret::generate())).await;
    let mut state = retry.state.clone();
    wait_state(&mut state, State::Rejected).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !retry.task.is_finished(),
        "default policy keeps retrying after a rejection"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_busy_server_is_not_taken_for_a_wrong_secret() {
    let secret = Secret::generate();
    let mut s = server_settings(vec![secret.clone()]);
    s.max_handshakes = 1;
    let srv = start_server(s).await;
    let hog = tokio::net::TcpStream::connect(srv.addr).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut c = client_settings(srv.addr, secret);
    c.exit_on_rejected = true;
    c.reject_confirm = Duration::from_secs(5);
    let mut cli = start_client(c).await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(
        !cli.task.is_finished(),
        "a full server must not end the client"
    );
    assert_ne!(*cli.state.borrow(), State::Rejected);
    drop(hog);
    wait_state(&mut cli.state, State::Connected).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_machine_with_the_same_name_does_not_take_over() {
    let echo = echo_server().await;
    let t = tunnel(vec![("web", service(echo, Protocol::Tcp))]).await;
    let web = t.server.services["web"];
    let mut open = open_echo(web).await;
    let first = t.server.shared.status().clients[0].from.clone();
    let mut copy = start_client(client_settings(t.server.addr, t.secret.clone())).await;
    let state = copy.state.clone();
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(*state.borrow(), State::Connecting, "the copy got in");
    assert_eq!(*t.client.state.borrow(), State::Connected);
    assert_eq!(t.server.shared.status().clients[0].from, first);
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    open.write_all(b"ping").await.unwrap();
    let mut back = [0u8; 4];
    open.read_exact(&mut back).await.unwrap();
    assert_eq!(&back, b"ping");
    assert_eq!(roundtrip(web, b"still").await, b"still");
    t.client.stop.send(true).unwrap();
    wait_state(&mut copy.state, State::Connected).await;
}

struct Freezer {
    addr: SocketAddr,
    frozen: Arc<std::sync::atomic::AtomicBool>,
    stuck_open: Arc<std::sync::atomic::AtomicUsize>,
}

async fn freezer(to: SocketAddr) -> Freezer {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let frozen = Arc::new(AtomicBool::new(false));
    let stuck_open = Arc::new(AtomicUsize::new(0));
    let (f, open) = (frozen.clone(), stuck_open.clone());
    tokio::spawn(async move {
        loop {
            let Ok((mut down, _)) = l.accept().await else {
                return;
            };
            let (f, open) = (f.clone(), open.clone());
            tokio::spawn(async move {
                let Ok(mut up) = tokio::net::TcpStream::connect(to).await else {
                    return;
                };
                let mut a = vec![0u8; 65536];
                let mut b = vec![0u8; 65536];
                loop {
                    if f.load(Ordering::SeqCst) {
                        drop(up);
                        open.fetch_add(1, Ordering::SeqCst);
                        while !matches!(down.read(&mut a).await, Ok(0) | Err(_)) {}
                        open.fetch_sub(1, Ordering::SeqCst);
                        return;
                    }
                    tokio::select! {
                        r = down.read(&mut a) => match r {
                            Ok(0) | Err(_) => return,
                            Ok(n) => if up.write_all(&a[..n]).await.is_err() { return },
                        },
                        r = up.read(&mut b) => match r {
                            Ok(0) | Err(_) => return,
                            Ok(n) => if down.write_all(&b[..n]).await.is_err() { return },
                        },
                        _ = tokio::time::sleep(Duration::from_millis(50)) => {}
                    }
                }
            });
        }
    });
    Freezer {
        addr,
        frozen,
        stuck_open,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_data_channels_end_with_their_session_even_when_no_close_arrives() {
    use std::sync::atomic::Ordering;
    if ws_mode() {
        return;
    }
    let secret = Secret::generate();
    let server = start_server(server_settings(vec![secret.clone()])).await;
    let f = freezer(server.addr).await;
    let mut cli = start_client(client_settings(f.addr, secret)).await;
    wait_state(&mut cli.state, State::Connected).await;
    let shared = server.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 2).await;
    f.frozen.store(true, Ordering::SeqCst);
    wait_state(&mut cli.state, State::Connecting).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let stuck = f.stuck_open.load(Ordering::SeqCst);
    assert!(stuck >= 1, "nothing was frozen");
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        while f.stuck_open.load(Ordering::SeqCst) > 0 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(
        closed.is_ok(),
        "{} idle data channels of the ended session stayed open",
        f.stuck_open.load(Ordering::SeqCst)
    );
}

struct Hanging;

impl Resolver for Hanging {
    fn resolve<'a>(&'a self, host_port: &'a str) -> Resolved<'a> {
        Box::pin(async move {
            if host_port.starts_with("hangs.test") {
                std::future::pending::<()>().await;
            }
            resolve(host_port).await
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_target_lookup_that_hangs_fails_within_the_connect_timeout() {
    let secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    let mut svc = service("127.0.0.1:1".parse().unwrap(), Protocol::Tcp);
    svc.target = "hangs.test:80".into();
    settings.services.insert("web".into(), svc);
    let server = start_server(settings).await;
    let mut c = client_settings(server.addr, secret);
    c.connect_timeout = Duration::from_secs(1);
    let mut cli =
        start_client_with(Client::new(over_transport(c)).with_resolver(Arc::new(Hanging))).await;
    wait_state(&mut cli.state, State::Connected).await;
    let shared = server.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 1).await;
    let started = std::time::Instant::now();
    let got = try_roundtrip(server.services["web"], b"x").await;
    assert_ne!(got.as_deref(), Some(&b"x"[..]));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the connection waited {:?} for a lookup",
        started.elapsed()
    );
}
