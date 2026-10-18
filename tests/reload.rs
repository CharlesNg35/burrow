mod common;

use std::net::SocketAddr;
use std::time::Duration;

use burrow::allowlist::AllowList;
use burrow::client::{Client, State};
use burrow::config::model::Protocol;
use burrow::config::settings::{ClientSettings, Forward, VisitorSettings};
use burrow::protocol::VisitTarget;
use burrow::secret::Secret;
use common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

async fn echoes(conn: &mut TcpStream) -> bool {
    tokio::time::timeout(Duration::from_secs(3), async {
        conn.write_all(b"ping").await.ok()?;
        let mut back = [0u8; 4];
        conn.read_exact(&mut back).await.ok()?;
        (&back == b"ping").then_some(())
    })
    .await
    .is_ok_and(|r| r.is_some())
}

async fn eventually(addr: SocketAddr) -> bool {
    for _ in 0..50 {
        if try_roundtrip(addr, b"hello").await.as_deref() == Some(b"hello") {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

struct Live {
    server: RunningServer,
    server_settings: burrow::config::settings::ServerSettings,
    client: RunningClient,
    reload: mpsc::Sender<ClientSettings>,
    settings: ClientSettings,
    secret: Secret,
}

impl Live {
    async fn start(allow: &str) -> Live {
        Live::start_with(allow, |_| {}).await
    }

    async fn start_with(
        allow: &str,
        edit: impl FnOnce(&mut burrow::config::settings::ServerSettings),
    ) -> Live {
        logs();
        let echo = echo_server().await;
        let secret = Secret::generate();
        let mut server = server_settings(vec![secret.clone()]);
        server
            .services
            .insert("web".into(), service(echo, Protocol::Tcp));
        edit(&mut server);
        let server_settings = server.clone();
        let server = start_server(server).await;
        let mut settings = over_transport(client_settings(server.addr, secret.clone()));
        settings.allow = AllowList::Rules(vec![allow.parse().unwrap()]);
        let (reload, reloads) = mpsc::channel(4);
        let mut client =
            start_client_with(Client::new(settings.clone()).with_reloads(reloads)).await;
        wait_state(&mut client.state, State::Connected).await;
        let shared = server.shared.clone();
        wait_for(|| shared.idle_channels("home") >= 1).await;
        Live {
            server,
            server_settings,
            client,
            reload,
            settings,
            secret,
        }
    }

    fn web(&self) -> SocketAddr {
        self.server.services["web"]
    }

    async fn change(&mut self, edit: impl FnOnce(&mut ClientSettings)) {
        edit(&mut self.settings);
        self.reload.send(self.settings.clone()).await.unwrap();
    }

    fn connected_secs(&self) -> u64 {
        self.server.shared.status().clients[0]
            .connected_secs
            .unwrap_or(0)
    }
}

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

fn free_local() -> SocketAddr {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
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

    let db_at = free_local();
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

    let web_at = free_local();
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

    let moved = free_local();
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
        Forward::single(free_local(), VisitTarget::service("db")),
    );
    let visitor = burrow::visitor::Visitor::bind(settings.clone()).unwrap();
    let (reload, reloads) = mpsc::channel(4);
    let (_stop, stop_rx) = tokio::sync::watch::channel(false);
    let _task = tokio::spawn(visitor.with_reloads(reloads).run(stop_rx));
    let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
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

mod process {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};

    pub struct Running {
        pub child: Child,
        pub log: PathBuf,
    }

    impl Running {
        pub fn start(role: &str, config: &Path, locks: &Path) -> Running {
            let log = config.with_extension("log");
            let child = Command::new(env!("CARGO_BIN_EXE_burrow"))
                .arg(role)
                .arg("-c")
                .arg(config)
                .env("BURROW_INSTANCE_DIR", locks)
                .env("BURROW_LOG", "burrow=info")
                .stdout(Stdio::null())
                .stderr(std::fs::File::create(&log).unwrap())
                .spawn()
                .unwrap();
            Running { child, log }
        }

        pub fn log(&self) -> String {
            std::fs::read_to_string(&self.log).unwrap_or_default()
        }

        pub async fn logged(&self, text: &str) -> bool {
            for _ in 0..100 {
                if self.log().contains(text) {
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            false
        }
    }

    impl Drop for Running {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    pub fn write(path: &Path, text: &str) {
        burrow::instance::write_private(path, text).unwrap();
    }

    pub fn server_line(addr: SocketAddr) -> String {
        if ws_mode() {
            format!("ws://{addr}/burrow")
        } else {
            addr.to_string()
        }
    }
}

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
    burrow::instance::write_private(
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
    tokio::spawn(burrow::reload::follow(
        config.clone(),
        (*settings).clone(),
        burrow::run::Env::default(),
        burrow::reload::accept(),
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

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_server_whose_stderr_went_away_keeps_running_and_reloading() {
    use std::process::{Command, Stdio};
    let dir = tempfile::tempdir().unwrap();
    let locks = tempfile::tempdir().unwrap();
    let (listen, web) = (free_local(), free_local());
    let config = dir.path().join("server.yaml");
    let yaml = |services: &str| {
        format!(
            "listen: {listen}\nclients:\n  home: {{ secret: {} }}\nservices:{services}\n",
            Secret::generate().encode()
        )
    };
    process::write(&config, &yaml(" {}"));
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let mut child = Command::new(env!("CARGO_BIN_EXE_burrow"))
        .args(["server", "-c"])
        .arg(&config)
        .env("BURROW_INSTANCE_DIR", locks.path())
        .env("BURROW_LOG", "burrow=info")
        .stdout(Stdio::null())
        .stderr(writer)
        .spawn()
        .unwrap();
    assert!(eventually_listening(listen).await);
    let pid = rustix::process::Pid::from_raw(child.id() as i32).unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::HUP).unwrap();
    process::write(
        &config,
        &yaml(&format!(
            "\n  web: {{ client: home, bind: {web}, target: 127.0.0.1:9 }}"
        )),
    );
    let listening = eventually_listening(web).await;
    let alive = child.try_wait().unwrap().is_none();
    let _ = child.kill();
    let _ = child.wait();
    assert!(alive, "the server died");
    assert!(listening, "the reload was not applied");
}

#[cfg(unix)]
async fn eventually_listening(addr: SocketAddr) -> bool {
    for _ in 0..100 {
        if TcpStream::connect(addr).await.is_ok() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

#[cfg(unix)]
#[test]
fn a_failing_command_whose_stderr_went_away_exits_with_its_code() {
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_burrow"))
        .args(["server", "-c", "/nonexistent/burrow/server.yaml"])
        .stdout(std::process::Stdio::null())
        .stderr(writer)
        .status()
        .unwrap();
    let code = status.code();
    assert!(code.is_some_and(|c| c != 0 && c != 101), "{status:?}");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sighup_as_soon_as_the_port_is_open_does_not_kill_the_server() {
    use std::process::{Command, Stdio};
    let dir = tempfile::tempdir().unwrap();
    let locks = tempfile::tempdir().unwrap();
    let listen = free_local();
    let config = dir.path().join("server.yaml");
    process::write(
        &config,
        &format!(
            "listen: {listen}\nclients:\n  home: {{ secret: {} }}\nservices: {{}}\n",
            Secret::generate().encode()
        ),
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_burrow"))
        .args(["server", "-c"])
        .arg(&config)
        .env("BURROW_INSTANCE_DIR", locks.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let pid = rustix::process::Pid::from_raw(child.id() as i32).unwrap();
    let up = eventually_listening(listen).await;
    for _ in 0..20 {
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::HUP);
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let alive = child.try_wait().unwrap().is_none();
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        up && alive,
        "a SIGHUP once the port is open killed the server"
    );
}
