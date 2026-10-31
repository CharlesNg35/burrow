use std::time::Duration;

use burrow::client::State;
use burrow::config::settings::{Forward, VisitorSettings};
use burrow::policy::allowlist::AllowList;
use burrow::wire::protocol::VisitTarget;
use burrow::wire::secret::Secret;
use burrow_config::Protocol;
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use super::common::*;
use super::{Live, echoes, eventually};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_allow_list_is_used_without_a_restart() {
    let mut t = Live::start("10.0.0.0/8").await;
    assert_ne!(
        try_roundtrip(t.web(), b"x").await.as_deref(),
        Some(&b"x"[..])
    );
    t.change(|s| s.allow = AllowList::Rules(vec!["127.0.0.0/8".parse().unwrap()]))
        .await;
    assert!(
        eventually(t.web()).await,
        "the widened allow list was not used"
    );
    t.change(|s| s.allow = AllowList::Rules(vec!["10.0.0.0/8".parse().unwrap()]))
        .await;
    let web = t.web();
    let mut refused = false;
    for _ in 0..50 {
        if try_roundtrip(web, b"x").await.as_deref() != Some(&b"x"[..]) {
            refused = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(refused, "the narrowed allow list was not used");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_live_change_keeps_the_session_and_open_connections() {
    let mut t = Live::start("127.0.0.0/8").await;
    let mut open = open_echo(t.web()).await;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let before = t.connected_secs();
    t.change(|s| {
        s.connect_timeout = Duration::from_secs(3);
        s.tcp.nodelay = false;
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(echoes(&mut open).await, "the open connection was cut");
    assert!(t.connected_secs() >= before, "the client reconnected");
    assert_eq!(*t.client.state.borrow(), State::Connected);
    assert!(eventually(t.web()).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reconnect_keeps_connections_that_are_open() {
    let mut t = Live::start("127.0.0.0/8").await;
    let mut open = open_echo(t.web()).await;
    t.change(|s| {
        s.allow = AllowList::Rules(vec![
            "127.0.0.0/8".parse().unwrap(),
            "10.0.0.0/8".parse().unwrap(),
        ])
    })
    .await;
    let shared = t.server.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 1).await;
    assert!(eventually(t.web()).await);
    assert!(
        echoes(&mut open).await,
        "a reconnect must not cut what is open"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_narrower_allow_list_closes_only_the_connections_it_no_longer_permits() {
    let mut t = Live::start("127.0.0.0/8").await;
    let mut open = open_echo(t.web()).await;
    t.change(|s| s.allow = AllowList::Rules(vec!["127.0.0.1/32".parse().unwrap()]))
        .await;
    let shared = t.server.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 1).await;
    assert!(
        echoes(&mut open).await,
        "a connection still allowed was cut"
    );
    t.change(|s| s.allow = AllowList::Rules(vec!["10.0.0.0/8".parse().unwrap()]))
        .await;
    assert_cut(open).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_client_moves_to_another_server() {
    let mut t = Live::start("127.0.0.0/8").await;
    let echo = echo_server().await;
    let mut other = server_settings(vec![t.secret.clone()]);
    other
        .services
        .insert("web".into(), service(echo, Protocol::Tcp));
    let other = start_server(other).await;
    let moved = over_transport(client_settings(other.addr, t.secret.clone()));
    t.change(|s| {
        s.server = moved.server.clone();
        s.endpoint = moved.endpoint.clone();
    })
    .await;
    let shared = other.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 1).await;
    assert!(eventually(other.services["web"]).await);
    let first = t.server.shared.clone();
    wait_for(|| first.session_names().is_empty()).await;
}

struct Visiting {
    reload: mpsc::Sender<VisitorSettings>,
    settings: VisitorSettings,
    _task: tokio::task::JoinHandle<()>,
    _stop: tokio::sync::watch::Sender<bool>,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn visitor_forwards_change_one_by_one() {
    logs();
    let db = echo_server().await;
    let web = echo_server().await;
    let secret = Secret::generate();
    let visitor_secret = Secret::generate();
    let mut server = server_settings(vec![secret.clone()]);
    server
        .visitors
        .insert("laptop".into(), visitor_auth(visitor_secret.clone(), &[]));
    server
        .services
        .insert("db".into(), private_service(db, &["laptop"]));
    server
        .services
        .insert("web".into(), private_service(web, &["laptop"]));
    let server = start_server(server).await;
    let mut client = start_client(client_settings(server.addr, secret)).await;
    wait_state(&mut client.state, State::Connected).await;
    let shared = server.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 1).await;

    let db_at = free_addr();
    let mut settings = visitor_settings(server.addr, visitor_secret, vec![]);
    settings.forwards.insert(
        "db".into(),
        Forward::single(db_at, VisitTarget::service("db")),
    );
    let visitor = burrow::visitor::Visitor::bind(settings.clone()).unwrap();
    let (reload, reloads) = mpsc::channel(4);
    let (stop, stop_rx) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(visitor.with_reloads(reloads).run(stop_rx));
    let mut v = Visiting {
        reload,
        settings,
        _task: task,
        _stop: stop,
    };
    let mut open = open_echo(db_at).await;

    let web_at = free_addr();
    v.settings.forwards.insert(
        "web".into(),
        Forward::single(web_at, VisitTarget::service("web")),
    );
    v.reload.send(v.settings.clone()).await.unwrap();
    assert!(eventually(web_at).await, "the new forward does not answer");
    assert!(
        echoes(&mut open).await,
        "an unchanged forward lost its connection"
    );

    let moved = free_addr();
    v.settings.forwards.get_mut("db").unwrap().local = moved;
    v.settings.forwards.remove("web");
    v.reload.send(v.settings.clone()).await.unwrap();
    assert!(eventually(moved).await, "the moved forward does not answer");
    let mut closed = false;
    for _ in 0..50 {
        if TcpStream::connect(web_at).await.is_err() && TcpStream::connect(db_at).await.is_err() {
            closed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(closed, "removed and moved forwards must stop listening");
    assert!(
        echoes(&mut open).await,
        "a moved forward keeps the connections it already had"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_visitor_forward_moved_to_a_busy_port_listens_once_it_is_free() {
    logs();
    let db = echo_server().await;
    let secret = Secret::generate();
    let visitor_secret = Secret::generate();
    let mut server = server_settings(vec![secret.clone()]);
    server
        .visitors
        .insert("laptop".into(), visitor_auth(visitor_secret.clone(), &[]));
    server
        .services
        .insert("db".into(), private_service(db, &["laptop"]));
    let server = start_server(server).await;
    let mut client = start_client(client_settings(server.addr, secret)).await;
    wait_state(&mut client.state, State::Connected).await;
    let mut settings = visitor_settings(server.addr, visitor_secret, vec![]);
    settings.forwards.insert(
        "db".into(),
        Forward::single(free_addr(), VisitTarget::service("db")),
    );
    let visitor = burrow::visitor::Visitor::bind(settings.clone()).unwrap();
    let (reload, reloads) = mpsc::channel(4);
    let (_stop, stop_rx) = tokio::sync::watch::channel(false);
    let _task = tokio::spawn(visitor.with_reloads(reloads).run(stop_rx));
    let busy = std::net::TcpListener::bind(free_addr()).unwrap();
    let moved = busy.local_addr().unwrap();
    settings.forwards.get_mut("db").unwrap().local = moved;
    reload.send(settings).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    drop(busy);
    let mut up = false;
    for _ in 0..30 {
        if try_roundtrip(moved, b"x").await.as_deref() == Some(&b"x"[..]) {
            up = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(
        up,
        "the moved forward never listened after its port was freed"
    );
}
