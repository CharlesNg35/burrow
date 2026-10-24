use std::net::SocketAddr;
use std::time::Duration;

use burrow::wire::protocol::VisitTarget;
use burrow::wire::secret::Secret;

use super::Setup;
use super::common::*;
use super::requests::narrowed;
use super::udp::udp_setup;

impl Setup {
    fn udp_visitor(
        &self,
        forwards: Vec<(&str, VisitTarget)>,
        change: impl FnOnce(&mut burrow::config::settings::VisitorSettings),
        reloads: bool,
    ) -> RunningVisitor {
        let mut settings =
            visitor_settings(self.tunnel.server.addr, self.visitor_secret.clone(), vec![]);
        for (name, to) in forwards {
            settings.forwards.insert(
                name.to_string(),
                udp_forward("127.0.0.1:0".parse().unwrap(), to),
            );
        }
        change(&mut settings);
        start_visitor_with(settings, reloads)
    }

    fn laptop_open(&self) -> u64 {
        self.tunnel
            .server
            .shared
            .status()
            .visitors
            .into_iter()
            .find(|v| v.name == "laptop")
            .map_or(0, |v| v.open)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_udp_forward_reaches_a_private_udp_service_and_closes_when_idle() {
    let s = udp_setup(|_, _| {}).await;
    let v = s.udp_visitor(
        vec![("dns", VisitTarget::service("dns"))],
        |v| v.udp_session_timeout = Duration::from_secs(1),
        false,
    );
    let at = v.addr("dns");
    let one = udp_bind().await;
    let two = udp_bind().await;
    assert!(udp_echoes_from(&one, at, b"from one").await);
    assert!(udp_echoes_from(&two, at, b"from two").await);
    assert!(udp_echoes_from(&one, at, &pattern(60_000, 9)).await);
    assert_eq!(
        s.laptop_open(),
        1,
        "one visit channel carries every peer of a port"
    );
    assert_eq!(s.tunnel.server.shared.udp_peers(), 2);
    assert_eq!(v.udp_peers.load(std::sync::atomic::Ordering::Acquire), 2);
    let shared = s.tunnel.server.shared.clone();
    wait_long(|| shared.status().visitors[0].open == 0).await;
    wait_for(|| shared.udp_peers() == 0).await;
    wait_for(|| v.udp_peers.load(std::sync::atomic::Ordering::Acquire) == 0).await;
    assert!(
        udp_echoes_from(&one, at, b"after idle").await,
        "the next datagram opens a new visit"
    );
    assert_eq!(s.laptop_open(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_udp_forward_forgets_its_peers_after_the_server_closes_the_visit_first() {
    let s = udp_setup(|srv, _| srv.udp_session_timeout = Duration::from_secs(1)).await;
    let v = s.udp_visitor(
        vec![("dns", VisitTarget::service("dns"))],
        |v| v.udp_session_timeout = Duration::from_secs(8),
        false,
    );
    let at = v.addr("dns");
    let one = udp_bind().await;
    assert!(udp_echoes_from(&one, at, b"once").await);
    let sent = std::time::Instant::now();
    assert_eq!(v.udp_peers.load(std::sync::atomic::Ordering::Acquire), 1);
    let shared = s.tunnel.server.shared.clone();
    wait_long(|| shared.status().visitors[0].open == 0).await;
    assert!(
        sent.elapsed() < Duration::from_secs(8),
        "the server closed the visit before the visitor's own timeout"
    );
    wait_long(|| v.udp_peers.load(std::sync::atomic::Ordering::Acquire) == 0).await;
    assert!(
        udp_echoes_from(&one, at, b"again").await,
        "the next datagram opens a new visit"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_udp_forward_reaches_a_target_through_reach() {
    let s = udp_setup(|srv, _| {
        srv.visitors.get_mut("laptop").unwrap().reach =
            [("home".to_string(), narrowed(&["127.0.0.0/8"]))].into();
    })
    .await;
    let v = s.udp_visitor(vec![("echo", s.reach())], |_| {}, false);
    assert!(udp_echoes_at(v.addr("echo"), b"reach over udp").await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_udp_forward_follows_a_new_service_target_on_its_next_datagram() {
    let mut settings = None;
    let s = udp_setup(|srv, _| settings = Some(srv.clone())).await;
    let v = s.udp_visitor(vec![("dns", VisitTarget::service("dns"))], |_| {}, false);
    let at = v.addr("dns");
    let local = udp_bind().await;
    assert!(udp_echoes_from(&local, at, b"old").await);
    let fresh = udp_bind().await;
    let fresh_at = fresh.local_addr().unwrap();
    let mut next = settings.unwrap();
    next.services
        .insert("dns".into(), private_udp_service(fresh_at, &["laptop"]));
    s.tunnel.server.reload(next).await;
    let mut buf = [0u8; 64];
    let got = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let _ = local.send_to(b"to the new target", at).await;
            if let Ok(Ok((n, _))) =
                tokio::time::timeout(Duration::from_millis(300), fresh.recv_from(&mut buf)).await
            {
                return n;
            }
        }
    })
    .await
    .expect("a datagram reaches the new target");
    assert_eq!(&buf[..got], b"to the new target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_udp_forward_dials_with_the_reloaded_secret() {
    let mut settings = None;
    let s = udp_setup(|srv, _| settings = Some(srv.clone())).await;
    let fresh = Secret::generate();
    let mut both = settings.unwrap();
    both.visitors.get_mut("laptop").unwrap().secrets =
        vec![s.visitor_secret.clone(), fresh.clone()];
    s.tunnel.server.reload(both.clone()).await;
    let shared = s.tunnel.server.shared.clone();
    wait_for(|| shared.settings_snapshot().visitors["laptop"].secrets.len() == 2).await;
    let at = SocketAddr::from(([127, 0, 0, 1], free_port()));
    let v = s.udp_visitor(
        vec![],
        |v| {
            v.forwards
                .insert("dns".into(), udp_forward(at, VisitTarget::service("dns")));
        },
        true,
    );
    let local = udp_bind().await;
    assert!(udp_echoes_from(&local, at, b"old secret").await);
    let first_visit = s.tunnel.server.shared.status().visitors[0].total;
    let mut next = visitor_settings(s.tunnel.server.addr, fresh.clone(), vec![]);
    next.forwards
        .insert("dns".into(), udp_forward(at, VisitTarget::service("dns")));
    v.reload.as_ref().unwrap().send(next).await.unwrap();
    let mut only_new = both;
    only_new.visitors.get_mut("laptop").unwrap().secrets = vec![fresh];
    s.tunnel.server.reload(only_new).await;
    wait_for(|| shared.settings_snapshot().visitors["laptop"].secrets.len() == 1).await;
    assert!(
        udp_echoes_from(&local, at, b"new secret").await,
        "the running forward reopens with the reloaded secret"
    );
    assert!(s.tunnel.server.shared.status().visitors[0].total > first_visit);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_udp_forward_keeps_at_most_its_udp_max_peers() {
    let s = udp_setup(|_, _| {}).await;
    let v = s.udp_visitor(
        vec![("dns", VisitTarget::service("dns"))],
        |v| v.udp_max_peers = 1,
        false,
    );
    let at = v.addr("dns");
    let one = udp_bind().await;
    let two = udp_bind().await;
    assert!(udp_echoes_from(&one, at, b"first").await);
    assert!(!udp_echoes_from(&two, at, b"second").await);
    assert!(udp_echoes_from(&one, at, b"first again").await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tcp_and_udp_forwards_share_a_port_number() {
    let s = udp_setup(|_, _| {}).await;
    let port = free_port();
    let at = SocketAddr::from(([127, 0, 0, 1], port));
    let mut settings = visitor_settings(s.tunnel.server.addr, s.visitor_secret.clone(), vec![]);
    settings.forwards.insert(
        "db".into(),
        burrow::config::settings::Forward::single(at, VisitTarget::service("db")),
    );
    settings
        .forwards
        .insert("dns".into(), udp_forward(at, VisitTarget::service("dns")));
    let _v = start_visitor(settings);
    assert!(udp_echoes_at(at, b"udp on the shared port").await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn burrow_visit_udp_forwards_a_service_named_on_the_command_line() {
    let s = udp_setup(|_, _| {}).await;
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("visitor.yaml");
    let server = if ws_mode() {
        format!("ws://{}/burrow", s.tunnel.server.addr)
    } else {
        s.tunnel.server.addr.to_string()
    };
    std::fs::write(
        &cfg,
        format!(
            "server: {server}\nname: laptop\nsecret: {}\nforwards: {{}}\n",
            s.visitor_secret.encode()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let burrow = |args: &[&str]| {
        let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_burrow"));
        c.arg("visit")
            .arg("-c")
            .arg(&cfg)
            .args(args)
            .env("BURROW_INSTANCE_DIR", dir.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped());
        c
    };
    let port = free_port();
    let mut child = burrow(&["--udp", "--service", "dns", "--local", &port.to_string()])
        .spawn()
        .unwrap();
    let echoed = udp_echoes_at(SocketAddr::from(([127, 0, 0, 1], port)), b"from the cli").await;
    let _ = child.kill();
    let _ = child.wait();
    assert!(echoed, "burrow visit --udp carries datagrams");
    for (args, says) in [
        (
            vec!["--udp", "--client", "home", "--socks", "1080"],
            "cannot be used with",
        ),
        (vec!["--udp"], "--udp needs --service NAME"),
        (
            vec![
                "--udp",
                "--client",
                "home",
                "--target",
                "unix:/run/a.sock",
                "--local",
                "9",
            ],
            "--udp cannot reach a Unix socket",
        ),
    ] {
        let out = burrow(&args).output().unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{args:?}");
        assert!(err.contains(says), "{args:?}: {err}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn datagrams_dropped_at_the_peer_cap_use_none_of_the_bandwidth() {
    let s = udp_setup(|srv, _| {
        srv.udp_max_peers = 1;
        srv.services.get_mut("dns").unwrap().bandwidth = Some(1000);
    })
    .await;
    let mut v = s.udp(VisitTarget::service("dns")).await.ok().unwrap();
    assert!(v.echoes(1, b"admitted").await);
    let big = vec![3u8; 1000];
    for _ in 0..100 {
        v.send(2, &big).await;
    }
    let mut back = 0;
    for _ in 0..8 {
        v.send(1, &big).await;
    }
    while let Got::Datagram(1, _) = v.next(Duration::from_secs(1)).await {
        back += 1;
    }
    assert_eq!(back, 8, "the admitted peer still has the whole burst");
}
