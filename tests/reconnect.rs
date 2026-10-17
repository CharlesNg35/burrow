mod common;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use burrow::client::{Client, RunEnd, State};
use burrow::config::model::Protocol;
use burrow::secret::Secret;
use burrow::transport::resolve::{Resolved, Resolver, resolve};
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
    c.endpoint = burrow::transport::endpoint::Endpoint::parse(&c.server).unwrap();
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
    moved.listen = Some("127.0.0.2:0".parse().unwrap());
    for svc in moved.services.values_mut() {
        svc.bind = Some("127.0.0.2:0".parse().unwrap());
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
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = free.local_addr().unwrap();
    drop(free);
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
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = free.local_addr().unwrap();
    drop(free);
    let mut c = client_settings(addr, Secret::generate());
    c.reconnect_max = Duration::from_millis(100);
    c.give_up_after = Some(Duration::from_secs(1));
    let cli = start_client(c).await;
    let end = tokio::time::timeout(Duration::from_secs(5), cli.task)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(end, RunEnd::GaveUp(d) if d >= Duration::from_secs(1)),
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
