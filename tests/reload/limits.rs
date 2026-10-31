use std::net::SocketAddr;
use std::time::Duration;

use burrow::client::State;
use burrow::config::settings::{ClientSettings, Limit};
use burrow::wire::secret::Secret;
use burrow_config::Protocol;
use tokio::net::TcpStream;

use super::common::*;
#[cfg(unix)]
use super::eventually;
use super::process;
use super::{Live, echoes};

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_warns_once_per_change_when_the_open_file_limit_lowers_its_limits() {
    use process::*;
    logs();
    let echo = echo_server().await;
    let home = Secret::generate();
    let mut server = server_settings(vec![home.clone()]);
    server
        .services
        .insert("web".into(), service(echo, Protocol::Tcp));
    let server = start_server(server).await;
    let web = server.services["web"];
    let dir = tempfile::tempdir().unwrap();
    let locks = tempfile::tempdir().unwrap();
    let yaml = |extra: &str| {
        format!(
            "server: {}\nname: home\nsecret: {}\nallow: [127.0.0.0/8]\n{extra}",
            server_line(server.addr),
            home.encode()
        )
    };
    let config = dir.path().join("client.yaml");
    write(&config, &yaml("max_connections: 1000\n"));
    let client = Running::start_with_open_files(1024, "client", &config, locks.path());
    let first = "max_connections 1000 is lowered to 192 by the open file limit (1024); ";
    assert!(client.logged(first).await, "{}", client.log());
    let service = if cfg!(target_os = "macos") {
        "install burrow as a service, which sets 10240"
    } else {
        "install burrow as a service, which sets 1048576"
    };
    assert!(
        client.log().lines().any(|l| l.contains(first)
            && (l.contains(service) || !cfg!(any(target_os = "linux", target_os = "macos")))),
        "{}",
        client.log()
    );
    let shared = server.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 1).await;
    assert!(eventually(web).await, "{}", client.log());

    write(
        &config,
        &yaml("max_connections: 1000\nconnect_timeout_secs: 5\n"),
    );
    assert!(
        client
            .logged("config reloaded: connect_timeout_secs applied")
            .await,
        "{}",
        client.log()
    );
    write(
        &config,
        &yaml("max_connections: 500\nconnect_timeout_secs: 5\n"),
    );
    assert!(
        client
            .logged("max_connections 500 is lowered to 192 by the open file limit (1024)")
            .await,
        "{}",
        client.log()
    );
    write(
        &config,
        &yaml("max_connections: 0\nconnect_timeout_secs: 5\n"),
    );
    assert!(
        client.logged("keeping the current config").await,
        "{}",
        client.log()
    );
    write(&config, &yaml("max_connections: 2\nmax_udp_peers: 100\n"));
    assert!(
        client
            .logged("max_udp_peers 100 is lowered to 64 by the open file limit (1024)")
            .await,
        "{}",
        client.log()
    );
    assert!(
        client.log().contains(
            "config reloaded: connect_timeout_secs, max_connections, max_udp_peers applied"
        ),
        "{}",
        client.log()
    );
    let log = client.log();
    assert_eq!(
        log.matches("max_connections 1000 is lowered to").count(),
        1,
        "{log}"
    );
    assert_eq!(
        log.matches("max_connections 500 is lowered to").count(),
        1,
        "{log}"
    );
    assert_eq!(
        log.matches("max_udp_peers 8192").count(),
        0,
        "the default max_udp_peers is lowered quietly: {log}"
    );
    assert_eq!(
        log.matches("max_udp_peers 100 is lowered to 64").count(),
        1,
        "{log}"
    );
    assert_eq!(log.matches("is lowered to").count(), 3, "{log}");
    assert!(
        log.contains("must be between 1 and 1000000, found 0"),
        "{log}"
    );
    assert!(eventually(web).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rejected_max_connections_keeps_the_running_limit() {
    use process::*;
    logs();
    let echo = echo_server().await;
    let home = Secret::generate();
    let mut server = server_settings(vec![home.clone()]);
    server
        .services
        .insert("web".into(), service(echo, Protocol::Tcp));
    let server = start_server(server).await;
    let web = server.services["web"];
    let dir = tempfile::tempdir().unwrap();
    let locks = tempfile::tempdir().unwrap();
    let yaml = |max: &str| {
        format!(
            "server: {}\nname: home\nsecret: {}\nallow: [127.0.0.0/8]\nmax_connections: {max}\n",
            server_line(server.addr),
            home.encode()
        )
    };
    let config = dir.path().join("client.yaml");
    write(&config, &yaml("2"));
    let client = Running::start("client", &config, locks.path());
    let shared = server.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 1).await;
    let _open = opens_exactly(web, 2).await;
    for (i, bad) in ["1000001", "lots"].into_iter().enumerate() {
        write(&config, &yaml(bad));
        let rejected = || client.log().matches("keeping the current config").count() > i;
        for _ in 0..100 {
            if rejected() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(rejected(), "{}", client.log());
        assert!(
            try_open(web).await.is_none(),
            "{bad} changed the running limit"
        );
    }
}

async fn try_open(addr: SocketAddr) -> Option<TcpStream> {
    let mut conn = TcpStream::connect(addr).await.ok()?;
    echoes(&mut conn).await.then_some(conn)
}

async fn opens_exactly(addr: SocketAddr, n: usize) -> Vec<TcpStream> {
    let mut open = Vec::new();
    for i in 0..n {
        open.push(
            try_open(addr)
                .await
                .unwrap_or_else(|| panic!("connection {} of {n} was refused", i + 1)),
        );
    }
    assert!(
        try_open(addr).await.is_none(),
        "connection {} got through a limit of {n}",
        n + 1
    );
    open
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_client_max_connections_follows_reloads_without_cutting_open_connections() {
    let mut t = Live::start_both(
        "127.0.0.0/8",
        |_| {},
        |c| c.max_connections = Limit::written(2),
    )
    .await;
    let web = t.web();
    let before = t.connected_secs();
    let mut open = opens_exactly(web, 2).await;

    t.change(|s| s.max_connections = Limit::written(3)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    open.extend(opens_exactly(web, 1).await);

    t.change(|s| s.max_connections = Limit::written(1)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    for conn in &mut open {
        assert!(
            echoes(conn).await,
            "lowering the limit cut an open connection"
        );
    }
    assert!(
        try_open(web).await.is_none(),
        "a lowered limit let a new one through"
    );
    open.truncate(1);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        try_open(web).await.is_none(),
        "one open is the whole limit of 1"
    );
    open.clear();
    let shared = t.server.shared.clone();
    wait_for(|| shared.status().clients[0].open == 0).await;
    let one = opens_exactly(web, 1).await;
    drop(one);
    wait_for(|| shared.status().clients[0].open == 0).await;

    t.change(|s| s.max_connections = Limit::written(3)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _three = opens_exactly(web, 3).await;
    assert!(
        t.connected_secs() >= before,
        "a max_connections change reconnected"
    );
    assert_eq!(*t.client.state.borrow(), State::Connected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_opens_during_a_max_connections_reload_stay_under_the_cap_and_leave_no_count_behind()
 {
    #[cfg(unix)]
    {
        use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
        let limit = getrlimit(Resource::Nofile);
        let _ = setrlimit(
            Resource::Nofile,
            Rlimit {
                current: limit.maximum.map(|m| m.min(8192)),
                maximum: limit.maximum,
            },
        );
    }
    let mut t = Live::start_both(
        "127.0.0.0/8",
        |_| {},
        |c| c.max_connections = Limit::written(20),
    )
    .await;
    let web = t.web();
    let burst = |n: usize| {
        (0..n)
            .map(|_| tokio::spawn(try_open(web)))
            .collect::<Vec<_>>()
    };
    let mut held = Vec::new();
    for task in burst(150) {
        held.extend(task.await.unwrap());
    }
    assert!(held.len() <= 20, "{} got through a limit of 20", held.len());
    let second = burst(150);
    t.change(|s| s.max_connections = Limit::written(40)).await;
    let third = burst(150);
    for task in second.into_iter().chain(third) {
        held.extend(task.await.unwrap());
    }
    assert!(held.len() <= 40, "{} got through a limit of 40", held.len());
    assert!(t.server.shared.status().clients[0].open <= 40);
    drop(held);
    let shared = t.server.shared.clone();
    wait_for(|| shared.status().clients[0].open == 0).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    drop(opens_exactly(web, 40).await);
}

async fn udp_peer(to: SocketAddr) -> tokio::net::UdpSocket {
    let s = udp_bind().await;
    s.connect(to).await.unwrap();
    s
}

async fn answers(peer: &tokio::net::UdpSocket, tries: usize) -> bool {
    let mut buf = [0u8; 64];
    let me = peer.local_addr().unwrap().to_string();
    for _ in 0..tries {
        peer.send(me.as_bytes()).await.unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_millis(200);
        while let Ok(Ok(n)) = tokio::time::timeout_at(deadline, peer.recv(&mut buf)).await {
            if &buf[..n] == me.as_bytes() {
                return true;
            }
        }
    }
    false
}

async fn with_dns(
    edit: impl FnOnce(&mut burrow::config::settings::ServerSettings),
    edit_client: impl FnOnce(&mut ClientSettings),
) -> (Live, SocketAddr) {
    let echo = udp_echo_server().await;
    let t = Live::start_both(
        "127.0.0.0/8",
        |s| {
            s.services
                .insert("dns".into(), service(echo, Protocol::Udp));
            edit(s);
        },
        edit_client,
    )
    .await;
    let dns = t.server.services["dns"];
    (t, dns)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_client_max_udp_peers_follows_reloads_without_dropping_known_peers() {
    let (mut t, dns) = with_dns(|_| {}, |c| c.max_udp_peers = Limit::written(2)).await;
    let (a, b, c, d, e) = (
        udp_peer(dns).await,
        udp_peer(dns).await,
        udp_peer(dns).await,
        udp_peer(dns).await,
        udp_peer(dns).await,
    );
    assert!(answers(&a, 25).await && answers(&b, 25).await);
    assert!(
        !answers(&c, 5).await,
        "a third peer got through max_udp_peers 2"
    );

    t.change(|s| s.max_udp_peers = Limit::written(3)).await;
    assert!(
        answers(&c, 25).await,
        "raising max_udp_peers did not let a peer in"
    );

    t.change(|s| s.max_udp_peers = Limit::written(1)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    for known in [&a, &b, &c] {
        assert!(
            answers(known, 10).await,
            "lowering max_udp_peers dropped a known peer"
        );
    }
    assert!(
        !answers(&d, 5).await,
        "a lowered max_udp_peers let a new peer in"
    );

    t.change(|s| s.udp_session_timeout = Duration::from_secs(2))
        .await;
    assert!(
        answers(&d, 75).await,
        "the peers that expired still count against max_udp_peers"
    );
    assert!(!answers(&e, 3).await, "one peer is the whole limit of 1");
    assert_eq!(*t.client.state.borrow(), State::Connected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_server_udp_max_peers_follows_reloads_without_dropping_known_peers() {
    let (t, dns) = with_dns(|s| s.udp_max_peers = 2, |_| {}).await;
    let (a, b, c, d, e) = (
        udp_peer(dns).await,
        udp_peer(dns).await,
        udp_peer(dns).await,
        udp_peer(dns).await,
        udp_peer(dns).await,
    );
    assert!(answers(&a, 25).await && answers(&b, 25).await);
    assert!(
        !answers(&c, 5).await,
        "a third peer got through udp.max_peers 2"
    );

    let mut next = t.server_settings.clone();
    next.udp_max_peers = 3;
    t.server.reload(next.clone()).await;
    assert!(
        answers(&c, 25).await,
        "raising udp.max_peers did not let a peer in"
    );

    next.udp_max_peers = 1;
    t.server.reload_applied(next.clone()).await;
    for known in [&a, &b, &c] {
        assert!(
            answers(known, 10).await,
            "lowering udp.max_peers dropped a known peer"
        );
    }
    assert!(
        !answers(&d, 5).await,
        "a lowered udp.max_peers let a new peer in"
    );
    assert_eq!(t.server.shared.status().udp_peers, 3);

    next.udp_session_timeout = Duration::from_secs(2);
    t.server.reload(next).await;
    let shared = t.server.shared.clone();
    for _ in 0..150 {
        if shared.status().udp_peers == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        shared.status().udp_peers,
        0,
        "the known peers did not expire"
    );
    assert!(
        answers(&d, 25).await,
        "no new peer got in after the others expired"
    );
    assert!(
        !answers(&e, 3).await,
        "a second new peer got through the lowered udp.max_peers 1"
    );
    assert_eq!(shared.status().udp_peers, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_udp_peers_during_a_max_udp_peers_reload_stay_under_the_cap() {
    #[cfg(unix)]
    {
        use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
        let limit = getrlimit(Resource::Nofile);
        let _ = setrlimit(
            Resource::Nofile,
            Rlimit {
                current: limit.maximum.map(|m| m.min(8192)),
                maximum: limit.maximum,
            },
        );
    }
    let (mut t, dns) = with_dns(|_| {}, |c| c.max_udp_peers = Limit::written(20)).await;
    let burst = |n: usize| {
        (0..n)
            .map(|_| {
                tokio::spawn(async move {
                    let peer = udp_peer(dns).await;
                    answers(&peer, 5).await.then_some(peer)
                })
            })
            .collect::<Vec<_>>()
    };
    let mut admitted = Vec::new();
    for task in burst(150) {
        admitted.extend(task.await.unwrap());
    }
    assert!(
        admitted.len() <= 20,
        "{} peers got through a limit of 20",
        admitted.len()
    );
    let second = burst(150);
    t.change(|s| s.max_udp_peers = Limit::written(40)).await;
    let third = burst(150);
    for task in second.into_iter().chain(third) {
        admitted.extend(task.await.unwrap());
    }
    assert!(
        admitted.len() <= 40,
        "{} peers got through a limit of 40",
        admitted.len()
    );
    assert!(admitted.len() > 20, "raising the limit let no peer in");
    for peer in &admitted {
        assert!(
            answers(peer, 10).await,
            "an admitted peer stopped getting answers"
        );
    }
}
