use std::time::Duration;

use burrow::client::{Client, State};
use burrow::policy::allowlist::AllowList;
use burrow::wire::secret::Secret;
use burrow_config::Protocol;
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use super::common::*;
use super::live::free_local;
use super::process;
use super::{Live, echoes, eventually};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_running_client_follows_its_file_and_keeps_its_identity() {
    use process::*;
    logs();
    let echo = echo_server().await;
    let (home, other) = (Secret::generate(), Secret::generate());
    let mut server = server_settings(vec![home.clone()]);
    server.clients.insert(
        "other".into(),
        burrow::config::settings::ClientAuth {
            secrets: vec![other.clone()],
            max_connections: None,
        },
    );
    server
        .services
        .insert("web".into(), service(echo, Protocol::Tcp));
    let server = start_server(server).await;
    let web = server.services["web"];
    let dir = tempfile::tempdir().unwrap();
    let locks = tempfile::tempdir().unwrap();
    let yaml = |name: &str, secret: &Secret, allow: &str| {
        format!(
            "server: {}\nname: {name}\nsecret: {}\nallow: [{allow}]\n",
            server_line(server.addr),
            secret.encode()
        )
    };
    let config = dir.path().join("client.yaml");
    write(&config, &yaml("home", &home, "10.0.0.0/8"));
    let client = Running::start("client", &config, locks.path());
    let shared = server.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 1).await;
    assert_ne!(try_roundtrip(web, b"x").await.as_deref(), Some(&b"x"[..]));
    write(&config, &yaml("home", &home, "127.0.0.0/8"));
    assert!(
        client
            .logged("config reloaded: allow changed; reconnecting")
            .await,
        "{}",
        client.log()
    );
    assert!(eventually(web).await, "{}", client.log());

    let second = dir.path().join("other.yaml");
    write(&second, &yaml("other", &other, "127.0.0.0/8"));
    let other_client = Running::start("client", &second, locks.path());
    wait_for(|| shared.idle_channels("other") >= 1).await;
    write(&second, &yaml("home", &other, "127.0.0.0/8"));
    assert!(
        other_client.logged("is already running").await,
        "{}",
        other_client.log()
    );
    assert!(other_client.log().contains("keeping the current config"));
    assert!(server.shared.session_names().contains(&"other".to_string()));

    #[cfg(unix)]
    {
        let pid = rustix::process::Pid::from_raw(client.child.id() as i32).unwrap();
        rustix::process::kill_process(pid, rustix::process::Signal::HUP).unwrap();
        assert!(
            client.logged("config reloaded: nothing changed").await,
            "{}",
            client.log()
        );
        assert!(eventually(web).await);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_running_visitor_opens_a_forward_added_to_its_file() {
    use process::*;
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
    let dir = tempfile::tempdir().unwrap();
    let locks = tempfile::tempdir().unwrap();
    let config = dir.path().join("visitor.yaml");
    let head = format!(
        "server: {}\nname: laptop\nsecret: {}\nforwards:\n",
        server_line(server.addr),
        visitor_secret.encode()
    );
    let first = free_local();
    write(
        &config,
        &format!("{head}  db: {{ service: db, local: {first} }}\n"),
    );
    let visitor = Running::start("visit", &config, locks.path());
    assert!(eventually(first).await, "{}", visitor.log());
    let second = free_local();
    write(
        &config,
        &format!(
            "{head}  db: {{ service: db, local: {first} }}\n  db2: {{ service: db, local: {second} }}\n"
        ),
    );
    assert!(eventually(second).await, "{}", visitor.log());
    assert!(
        visitor.logged("config reloaded: forward db2 changed").await,
        "{}",
        visitor.log()
    );
    assert!(eventually(first).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_secret_starts_the_rejection_count_over() {
    logs();
    let server = start_server(server_settings(vec![Secret::generate()])).await;
    let mut settings = over_transport(client_settings(server.addr, Secret::generate()));
    settings.reconnect_initial = Duration::from_millis(400);
    settings.reconnect_max = Duration::from_millis(400);
    settings.reject_confirm = Duration::ZERO;
    settings.exit_on_rejected = true;
    let (reload, reloads) = mpsc::channel(4);
    let client = start_client_with(Client::new(settings.clone()).with_reloads(reloads)).await;
    let refused = || server.shared.status().handshakes_refused;
    let shared = server.shared.clone();
    wait_for(|| shared.status().handshakes_refused >= 2).await;
    settings.secret = Secret::generate();
    reload.send(settings).await.unwrap();
    let start = refused();
    for _ in 0..50 {
        if refused() > start {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(refused() > start, "no attempt with the new secret");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !client.task.is_finished(),
        "one rejection of the new secret must not count as confirmed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_live_change_counts_for_the_next_disconnect() {
    let mut t = Live::start("127.0.0.0/8").await;
    t.change(|s| s.exit_on_rejected = true).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut gone = server_settings(vec![]);
    gone.clients.clear();
    t.server.reload(gone).await;
    let end = tokio::time::timeout(Duration::from_secs(5), &mut t.client.task)
        .await
        .expect("the client stops at once")
        .unwrap();
    assert_eq!(end, burrow::client::RunEnd::Rejected);
    assert_eq!(
        t.server.shared.status().handshakes_refused,
        0,
        "the client must stop when removed, without trying again"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replacing_the_ca_file_reloads_the_client() {
    use burrow::config::{Loaded, Role, load_file};
    let dir = tempfile::tempdir().unwrap();
    let cert = |name: &str| {
        let key = rcgen::KeyPair::generate().unwrap();
        rcgen::CertificateParams::new(vec![name.to_string()])
            .unwrap()
            .self_signed(&key)
            .unwrap()
            .pem()
    };
    let ca = dir.path().join("ca.pem");
    std::fs::write(&ca, cert("one.example.com")).unwrap();
    let config = dir.path().join("client.yaml");
    burrow::base::fs::write_private(
        &config,
        &format!(
            "server: wss://one.example.com/burrow\nname: home\nsecret: {}\nallow: [127.0.0.0/8]\nca: ca.pem\n",
            Secret::generate().encode()
        ),
    )
    .unwrap();
    let nothing = |_: &str| None;
    let Some(Loaded::Client(settings, _)) = load_file(&config, Some(Role::Client), &nothing)
        .unwrap()
        .loaded
    else {
        panic!("client config loads")
    };
    assert!(settings.ca_file.is_some());
    let (tx, mut rx) = mpsc::channel(4);
    tokio::spawn(burrow::app::reload::follow(
        config.clone(),
        (*settings).clone(),
        burrow::config::env::Env::default(),
        burrow::app::reload::accept(),
        tx,
    ));
    tokio::time::sleep(Duration::from_millis(300)).await;
    std::fs::write(&ca, cert("two.example.com")).unwrap();
    let next = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("a new ca file reloads the config")
        .unwrap();
    assert_ne!(next.ca_pem, settings.ca_pem);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_handshake_limit_is_used_after_a_reload() {
    logs();
    let mut settings = server_settings(vec![Secret::generate()]);
    settings.max_handshakes = 1;
    settings.handshake_timeout = Duration::from_secs(10);
    let server = start_server(settings.clone()).await;
    let refused = || server.shared.status().handshakes_refused;
    let _first = TcpStream::connect(server.addr).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let _second = TcpStream::connect(server.addr).await.unwrap();
    let shared = server.shared.clone();
    wait_for(|| shared.status().handshakes_refused >= 1).await;
    settings.max_handshakes = 4;
    server.reload(settings).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let before = refused();
    let _third = TcpStream::connect(server.addr).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(refused(), before, "the raised limit was not used");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn turning_plaintext_on_reconnects_clients_instead_of_breaking_them_silently() {
    let mut t = Live::start("127.0.0.0/8").await;
    let mut next = t.server_settings.clone();
    next.insecure_plaintext = true;
    t.server.reload(next).await;
    let mut state = t.client.state.clone();
    tokio::time::timeout(Duration::from_secs(5), async {
        while *state.borrow_and_update() == State::Connected {
            state.changed().await.unwrap();
        }
    })
    .await
    .expect("the client is disconnected");
    t.change(|s| s.insecure_plaintext = true).await;
    wait_state(&mut t.client.state, State::Connected).await;
    assert!(
        eventually(t.web()).await,
        "no traffic after both sides changed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_client_connection_limit_counts_connections_kept_over_a_reconnect() {
    let mut t = Live::start_with("127.0.0.0/8", |s| {
        s.clients.get_mut("home").unwrap().max_connections = Some(2);
    })
    .await;
    let mut kept = vec![open_echo(t.web()).await, open_echo(t.web()).await];
    t.change(|s| {
        s.allow = AllowList::Rules(vec![
            "127.0.0.0/8".parse().unwrap(),
            "10.0.0.0/8".parse().unwrap(),
        ])
    })
    .await;
    let shared = t.server.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    for open in &mut kept {
        assert!(echoes(open).await, "a reconnect must not cut what is open");
    }
    assert_ne!(
        try_roundtrip(t.web(), b"third").await.as_deref(),
        Some(&b"third"[..]),
        "a third connection got through a limit of 2"
    );
    assert_eq!(t.server.shared.status().clients[0].open, 2);
    drop(kept);
    assert!(eventually(t.web()).await);
}
