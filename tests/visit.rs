mod common;

use std::net::SocketAddr;
use std::time::Duration;

use burrow::allowlist::{AllowList, Rule};
use burrow::config::settings::ServerSettings;
use burrow::dialer::{DialError, Dialer};
use burrow::protocol::{ChannelKind, Hello, Status, VisitRequest, VisitTarget};
use burrow::secret::Secret;
use common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Setup {
    tunnel: Tunnel,
    visitor_secret: Secret,
    echo: SocketAddr,
}

async fn setup(change: impl FnOnce(&mut ServerSettings, SocketAddr)) -> Setup {
    setup_with(change, |c| c).await
}

async fn setup_with(
    change: impl FnOnce(&mut ServerSettings, SocketAddr),
    client: impl FnOnce(
        burrow::config::settings::ClientSettings,
    ) -> burrow::config::settings::ClientSettings,
) -> Setup {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let visitor_secret = Secret::generate();
    let mut server = server_settings(vec![secret.clone()]);
    server
        .visitors
        .insert("laptop".into(), visitor_auth(visitor_secret.clone(), &[]));
    server
        .services
        .insert("db".into(), private_service(echo, &["laptop"]));
    change(&mut server, echo);
    let tunnel = tunnel_with(server, vec![], client, secret).await;
    Setup {
        tunnel,
        visitor_secret,
        echo,
    }
}

impl Setup {
    fn visitor(&self, forwards: Vec<(&str, VisitTarget)>) -> RunningVisitor {
        start_visitor(visitor_settings(
            self.tunnel.server.addr,
            self.visitor_secret.clone(),
            forwards,
        ))
    }

    fn reach(&self) -> VisitTarget {
        VisitTarget::Client {
            client: "home".into(),
            target: self.echo.to_string(),
        }
    }

    async fn ask(&self, check: bool, to: VisitTarget) -> Result<Status, &'static str> {
        ask(
            self.tunnel.server.addr,
            self.visitor_secret.clone(),
            check,
            to,
        )
        .await
    }
}

async fn ask(
    server: SocketAddr,
    secret: Secret,
    check: bool,
    to: VisitTarget,
) -> Result<Status, &'static str> {
    let dialer = Dialer::visitor(&visitor_settings(server, secret, vec![]));
    let mut ch = match dialer
        .open(ChannelKind::Visit, Hello::Visit { version: 1 })
        .await
    {
        Ok(ch) => ch,
        Err(DialError::Rejected) => return Err("rejected"),
        Err(DialError::Lost(_) | DialError::Unreachable(_)) => return Err("lost"),
    };
    ch.writer
        .send(
            &mut ch.stream,
            &VisitRequest {
                check,
                protocol: burrow_config::Protocol::Tcp,
                to,
            }
            .encode()
            .unwrap(),
        )
        .await
        .unwrap();
    match tokio::time::timeout(Duration::from_secs(10), ch.reader.recv(&mut ch.stream)).await {
        Ok(Ok(Some(b))) => Ok(Status::decode(b).unwrap()),
        _ => Err("no answer"),
    }
}

async fn refused(addr: SocketAddr) -> bool {
    let payload = pattern(1000, 3);
    try_roundtrip(addr, &payload).await.as_deref() != Some(&payload[..])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn visitor_reaches_a_private_service() {
    let s = setup(|_, _| {}).await;
    assert!(
        !s.tunnel.server.services.contains_key("db"),
        "a private service opens no port on the server"
    );
    let v = s.visitor(vec![("db", VisitTarget::service("db"))]);
    for (len, seed) in [(0, 1), (1, 2), (70_000, 3), (3_000_000, 4)] {
        let data = pattern(len, seed);
        assert_eq!(roundtrip(v.addr("db"), &data).await, data, "{len} bytes");
    }
    let many: Vec<_> = (0..20)
        .map(|i| {
            let addr = v.addr("db");
            tokio::spawn(async move {
                let data = pattern(50_000, i);
                assert_eq!(roundtrip(addr, &data).await, data);
            })
        })
        .collect();
    for t in many {
        t.await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_public_service_can_list_visitors_too() {
    let s = setup(|srv, echo| {
        let mut web = service(echo, burrow_config::Protocol::Tcp);
        web.visitors = vec!["laptop".into()];
        srv.services.insert("web".into(), web);
    })
    .await;
    let v = s.visitor(vec![("web", VisitTarget::service("web"))]);
    let data = pattern(10_000, 9);
    assert_eq!(roundtrip(v.addr("web"), &data).await, data);
    assert_eq!(
        roundtrip(s.tunnel.server.services["web"], &data).await,
        data
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reach_lets_a_visitor_pick_the_target() {
    let s = setup(|srv, _| {
        srv.visitors.get_mut("laptop").unwrap().reach =
            [("home".to_string(), burrow::allowlist::AllowList::All)].into();
    })
    .await;
    let v = s.visitor(vec![("echo", s.reach())]);
    let data = pattern(100_000, 5);
    assert_eq!(roundtrip(v.addr("echo"), &data).await, data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn visitors_only_get_what_the_server_grants() {
    let s = setup(|srv, echo| {
        srv.services
            .insert("other".into(), private_service(echo, &["someone"]));
        srv.visitors
            .insert("someone".into(), visitor_auth(Secret::generate(), &[]));
    })
    .await;
    assert_eq!(
        s.ask(true, VisitTarget::service("db")).await,
        Ok(Status::Ok)
    );
    for to in [
        VisitTarget::service("other"),
        VisitTarget::service("missing"),
        s.reach(),
    ] {
        assert_eq!(s.ask(true, to.clone()).await, Ok(Status::Forbidden), "{to}");
        assert_eq!(
            s.ask(false, to.clone()).await,
            Ok(Status::Forbidden),
            "{to}"
        );
    }
    let v = s.visitor(vec![
        ("other", VisitTarget::service("other")),
        ("echo", s.reach()),
    ]);
    assert!(refused(v.addr("other")).await);
    assert!(refused(v.addr("echo")).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wrong_secrets_and_client_secrets_are_rejected() {
    let s = setup(|_, _| {}).await;
    let db = || VisitTarget::service("db");
    assert_eq!(
        ask(s.tunnel.server.addr, Secret::generate(), true, db()).await,
        Err("rejected")
    );
    let mut as_client = visitor_settings(s.tunnel.server.addr, s.tunnel.secret.clone(), vec![]);
    as_client.name = "home".into();
    let dialer = Dialer::visitor(&as_client);
    assert!(matches!(
        dialer
            .open(ChannelKind::Visit, Hello::Visit { version: 1 })
            .await,
        Err(DialError::Rejected)
    ));
    let v = start_visitor(visitor_settings(
        s.tunnel.server.addr,
        Secret::generate(),
        vec![("db", db())],
    ));
    assert!(refused(v.addr("db")).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_client_allow_list_still_decides() {
    let s = setup_with(
        |srv, _| {
            srv.visitors.get_mut("laptop").unwrap().reach =
                [("home".to_string(), burrow::allowlist::AllowList::All)].into();
        },
        |mut c| {
            c.allow = AllowList::Rules(vec!["127.0.0.1/32:1".parse::<Rule>().unwrap()]);
            c
        },
    )
    .await;
    assert_eq!(s.ask(false, s.reach()).await, Ok(Status::Denied));
    assert_eq!(
        s.ask(false, VisitTarget::service("db")).await,
        Ok(Status::Denied)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_offline_client_is_reported() {
    let visitor_secret = Secret::generate();
    let mut server = server_settings(vec![Secret::generate()]);
    server
        .visitors
        .insert("laptop".into(), visitor_auth(visitor_secret.clone(), &[]));
    server.services.insert(
        "db".into(),
        private_service("127.0.0.1:9".parse().unwrap(), &["laptop"]),
    );
    let srv = start_server(server).await;
    let db = VisitTarget::service("db");
    assert_eq!(
        ask(srv.addr, visitor_secret.clone(), true, db.clone()).await,
        Ok(Status::Offline)
    );
    assert_eq!(
        ask(srv.addr, visitor_secret, false, db).await,
        Ok(Status::Offline)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connection_limits_apply_to_visitors() {
    let s = setup(|srv, _| {
        srv.visitors.get_mut("laptop").unwrap().max_connections = Some(1);
    })
    .await;
    let v = s.visitor(vec![("db", VisitTarget::service("db"))]);
    let mut held = tokio::net::TcpStream::connect(v.addr("db")).await.unwrap();
    held.write_all(b"ping").await.unwrap();
    let mut buf = [0u8; 4];
    held.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"ping");
    assert_eq!(
        s.ask(false, VisitTarget::service("db")).await,
        Ok(Status::Busy)
    );
    drop(held);
    let data = pattern(1000, 1);
    let mut ok = false;
    for _ in 0..50 {
        if try_roundtrip(v.addr("db"), &data).await.as_deref() == Some(&data[..]) {
            ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        ok,
        "the slot is free again once the first connection closes"
    );
}

async fn closed_after_reload(change: impl FnOnce(&mut ServerSettings)) {
    let s = setup(|srv, _| {
        srv.visitors.get_mut("laptop").unwrap().reach =
            [("home".to_string(), burrow::allowlist::AllowList::All)].into();
    })
    .await;
    let v = s.visitor(vec![
        ("db", VisitTarget::service("db")),
        ("echo", s.reach()),
    ]);
    let mut open = Vec::new();
    for name in ["db", "echo"] {
        let mut c = tokio::net::TcpStream::connect(v.addr(name)).await.unwrap();
        c.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        c.read_exact(&mut buf).await.unwrap();
        open.push(c);
    }
    let mut next = s.tunnel.server.shared.settings_snapshot();
    change(&mut next);
    s.tunnel.server.reload(next).await;
    let mut closed = 0;
    for mut c in open {
        let mut buf = [0u8; 16];
        let r = tokio::time::timeout(Duration::from_secs(5), c.read(&mut buf)).await;
        if matches!(r, Ok(Ok(0)) | Ok(Err(_))) {
            closed += 1;
        }
    }
    assert_eq!(closed, 2, "a revoked visitor's open connections are closed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reload_closes_revoked_visits() {
    closed_after_reload(|s| {
        s.visitors.remove("laptop");
    })
    .await;
    closed_after_reload(|s| {
        s.visitors.get_mut("laptop").unwrap().secrets = vec![Secret::generate()];
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reload_closes_each_revoked_permission() {
    let s = setup(|srv, _| {
        srv.visitors.get_mut("laptop").unwrap().reach =
            [("home".to_string(), burrow::allowlist::AllowList::All)].into();
    })
    .await;
    let v = s.visitor(vec![
        ("db", VisitTarget::service("db")),
        ("echo", s.reach()),
    ]);
    let connect = |name: &'static str| {
        let addr = v.addr(name);
        async move {
            let mut c = tokio::net::TcpStream::connect(addr).await.unwrap();
            c.write_all(b"ping").await.unwrap();
            let mut buf = [0u8; 4];
            c.read_exact(&mut buf).await.unwrap();
            c
        }
    };
    let is_closed = |mut c: tokio::net::TcpStream| async move {
        let mut buf = [0u8; 16];
        let r = tokio::time::timeout(Duration::from_secs(5), c.read(&mut buf)).await;
        matches!(r, Ok(Ok(0)) | Ok(Err(_)))
    };
    let still_open = |mut c: tokio::net::TcpStream| async move {
        c.write_all(b"more").await.unwrap();
        let mut buf = [0u8; 4];
        tokio::time::timeout(Duration::from_secs(5), c.read_exact(&mut buf))
            .await
            .is_ok_and(|r| r.is_ok())
    };

    let (db, echo) = (connect("db").await, connect("echo").await);
    let mut next = s.tunnel.server.shared.settings_snapshot();
    next.services.get_mut("db").unwrap().visitors = vec!["someone".into()];
    next.visitors
        .insert("someone".into(), visitor_auth(Secret::generate(), &[]));
    s.tunnel.server.reload(next.clone()).await;
    assert!(is_closed(db).await, "removed from the service's visitors");
    assert!(still_open(echo).await, "reach was not changed");

    let echo = connect("echo").await;
    next.visitors.get_mut("laptop").unwrap().reach.clear();
    s.tunnel.server.reload(next).await;
    assert!(is_closed(echo).await, "reach removed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn visits_work_with_insecure_plaintext() {
    let s = setup_with(
        |srv, _| srv.insecure_plaintext = true,
        |mut c| {
            c.insecure_plaintext = true;
            c
        },
    )
    .await;
    let v = s.visitor(vec![("db", VisitTarget::service("db"))]);
    for (len, seed) in [(10, 1), (1_000_000, 2)] {
        let data = pattern(len, seed);
        assert_eq!(roundtrip(v.addr("db"), &data).await, data);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalid_requests_are_answered_and_closed() {
    let s = setup(|srv, _| {
        srv.visitors.get_mut("laptop").unwrap().reach =
            [("home".to_string(), burrow::allowlist::AllowList::All)].into();
    })
    .await;
    assert_eq!(
        s.ask(
            false,
            VisitTarget::Client {
                client: "home".into(),
                target: "no-port".into()
            }
        )
        .await,
        Ok(Status::Invalid)
    );
    let dialer = Dialer::visitor(&visitor_settings(
        s.tunnel.server.addr,
        s.visitor_secret.clone(),
        vec![],
    ));
    let mut ch = dialer
        .open(ChannelKind::Visit, Hello::Visit { version: 1 })
        .await
        .ok()
        .unwrap();
    ch.writer
        .send(&mut ch.stream, b"\x20garbage")
        .await
        .unwrap();
    let answer = ch
        .reader
        .recv(&mut ch.stream)
        .await
        .unwrap()
        .unwrap()
        .to_vec();
    assert_eq!(Status::decode(&answer), Ok(Status::Invalid));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_busy_local_port_is_explained() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut settings = visitor_settings(
        "127.0.0.1:1".parse().unwrap(),
        Secret::generate(),
        vec![("db", VisitTarget::service("db"))],
    );
    settings.forwards.get_mut("db").unwrap().local = taken.local_addr().unwrap();
    let Err(f) = burrow::visitor::Visitor::bind(settings) else {
        panic!("binding a used port fails");
    };
    assert!(
        f.message
            .starts_with("cannot bind forward \"db\" on 127.0.0.1:"),
        "{}",
        f.message
    );
    assert!(
        f.message.ends_with("address already in use"),
        "{}",
        f.message
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oversized_requests_are_dropped_without_a_reply() {
    let s = setup(|_, _| {}).await;
    let dialer = Dialer::visitor(&visitor_settings(
        s.tunnel.server.addr,
        s.visitor_secret.clone(),
        vec![],
    ));
    let mut ch = dialer
        .open(ChannelKind::Visit, Hello::Visit { version: 1 })
        .await
        .ok()
        .unwrap();
    ch.writer.send(&mut ch.stream, &[0x20; 2000]).await.unwrap();
    let r = tokio::time::timeout(Duration::from_secs(5), ch.reader.recv(&mut ch.stream)).await;
    assert!(
        matches!(r, Ok(Ok(None)) | Ok(Err(_))),
        "closed without an answer"
    );
}

struct SlowName;

impl burrow::transport::resolve::Resolver for SlowName {
    fn resolve<'a>(&'a self, host_port: &'a str) -> burrow::transport::resolve::Resolved<'a> {
        Box::pin(async move {
            if host_port.starts_with("slow.test:") {
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            burrow::transport::resolve::resolve(host_port).await
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_that_leaves_mid_setup_is_reported_offline() {
    logs();
    let visitor_secret = Secret::generate();
    let secret = Secret::generate();
    let mut server = server_settings(vec![secret.clone()]);
    server.visitors.insert(
        "laptop".into(),
        visitor_auth(visitor_secret.clone(), &["home"]),
    );
    let srv = start_server(server).await;
    let cs = over_transport(client_settings(srv.addr, secret));
    let client = burrow::client::Client::new(cs).with_resolver(std::sync::Arc::new(SlowName));
    let mut cli = start_client_with(client).await;
    wait_state(&mut cli.state, burrow::client::State::Connected).await;
    let shared = srv.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 1).await;
    let stop = cli.stop.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let _ = stop.send(true);
    });
    let to = VisitTarget::Client {
        client: "home".into(),
        target: "slow.test:9".into(),
    };
    assert_eq!(
        ask(srv.addr, visitor_secret, false, to).await,
        Ok(Status::Offline)
    );
}

fn narrowed(rules: &[&str]) -> AllowList {
    AllowList::Rules(rules.iter().map(|r| r.parse::<Rule>().unwrap()).collect())
}

fn on_home(target: String) -> VisitTarget {
    VisitTarget::Client {
        client: "home".into(),
        target,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn narrowed_reach_admits_its_targets_and_refuses_the_rest() {
    let s = setup(|srv, _| {
        srv.visitors.get_mut("laptop").unwrap().reach =
            [("home".to_string(), narrowed(&["127.0.0.0/8"]))].into();
    })
    .await;
    let port = s.echo.port();
    assert_eq!(s.ask(false, s.reach()).await, Ok(Status::Ok));
    assert_eq!(
        s.ask(false, on_home("10.1.2.3:22".into())).await,
        Ok(Status::Forbidden),
        "the server refuses an address outside reach without asking the client"
    );
    assert_eq!(
        s.ask(false, on_home(format!("localhost:{port}"))).await,
        Ok(Status::Ok)
    );
    let v = s.visitor(vec![("byname", on_home(format!("localhost:{port}")))]);
    let data = pattern(50_000, 4);
    assert_eq!(roundtrip(v.addr("byname"), &data).await, data);

    let far = setup(|srv, _| {
        srv.visitors.get_mut("laptop").unwrap().reach =
            [("home".to_string(), narrowed(&["10.0.0.0/8"]))].into();
    })
    .await;
    let port = far.echo.port();
    assert_eq!(far.ask(false, far.reach()).await, Ok(Status::Forbidden));
    assert_eq!(
        far.ask(false, on_home(format!("localhost:{port}"))).await,
        Ok(Status::Denied),
        "the client refuses a name that resolves outside reach"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn narrowing_reach_on_reload_closes_connections_it_no_longer_covers() {
    let mut settings = None;
    let s = setup(|srv, _| {
        srv.visitors.get_mut("laptop").unwrap().reach =
            [("home".to_string(), AllowList::All)].into();
        settings = Some(srv.clone());
    })
    .await;
    let mut next = settings.unwrap();
    let v = s.visitor(vec![("echo", s.reach())]);
    let open = open_echo(v.addr("echo")).await;
    next.visitors.get_mut("laptop").unwrap().reach =
        [("home".to_string(), narrowed(&["10.0.0.0/8"]))].into();
    s.tunnel.server.reload(next).await;
    assert_cut(open).await;
    assert!(refused(v.addr("echo")).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_socks_forward_checks_its_client_without_a_target_under_narrowed_reach() {
    let s = setup(|srv, _| {
        srv.visitors.get_mut("laptop").unwrap().reach =
            [("home".to_string(), narrowed(&["127.0.0.0/8:22"]))].into();
    })
    .await;
    assert_eq!(s.ask(true, on_home(String::new())).await, Ok(Status::Ok));
    assert_eq!(
        s.ask(false, on_home(String::new())).await,
        Ok(Status::Invalid)
    );
    assert_eq!(
        s.ask(true, on_home("127.0.0.1:80".into())).await,
        Ok(Status::Forbidden),
        "an explicit target outside reach still fails its check"
    );
    let other = setup(|_, _| {}).await;
    assert_eq!(
        other.ask(true, on_home(String::new())).await,
        Ok(Status::Forbidden),
        "no reach for the client at all"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stopped_visitor_no_longer_listens() {
    let s = setup(|_, _| {}).await;
    let v = s.visitor(vec![("db", VisitTarget::service("db"))]);
    let addr = v.addr("db");
    assert_eq!(roundtrip(addr, b"up").await, b"up");
    v.stop.send(true).unwrap();
    v.task.await.unwrap();
    assert!(
        tokio::net::TcpStream::connect(addr).await.is_err(),
        "the port still accepts after the visitor stopped"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn widening_reach_keeps_open_visits() {
    for wider in ["wider", "any"] {
        let mut settings = None;
        let s = setup(|srv, echo| {
            srv.visitors.get_mut("laptop").unwrap().reach = [(
                "home".to_string(),
                narrowed(&[&format!("127.0.0.1/32:{}", echo.port())]),
            )]
            .into();
            settings = Some(srv.clone());
        })
        .await;
        let mut next = settings.unwrap();
        let v = s.visitor(vec![("echo", s.reach())]);
        let mut open = open_echo(v.addr("echo")).await;
        next.visitors.get_mut("laptop").unwrap().reach = [(
            "home".to_string(),
            if wider == "any" {
                AllowList::All
            } else {
                narrowed(&[&format!("127.0.0.1/32:{}", s.echo.port()), "10.0.0.0/8"])
            },
        )]
        .into();
        s.tunnel.server.reload(next).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        open.write_all(b"ping").await.unwrap();
        let mut back = [0u8; 4];
        let got = tokio::time::timeout(Duration::from_secs(3), open.read_exact(&mut back)).await;
        assert!(
            matches!(got, Ok(Ok(_))) && &back == b"ping",
            "{wider} reach cut an open visit"
        );
    }
}

async fn udp_setup(change: impl FnOnce(&mut ServerSettings, SocketAddr)) -> Setup {
    let echo = udp_echo_server().await;
    let secret = Secret::generate();
    let visitor_secret = Secret::generate();
    let mut server = server_settings(vec![secret.clone()]);
    server
        .visitors
        .insert("laptop".into(), visitor_auth(visitor_secret.clone(), &[]));
    server
        .services
        .insert("dns".into(), private_udp_service(echo, &["laptop"]));
    server
        .services
        .insert("db".into(), private_service(echo, &["laptop"]));
    change(&mut server, echo);
    let tunnel = tunnel_with(server, vec![], |c| c, secret).await;
    Setup {
        tunnel,
        visitor_secret,
        echo,
    }
}

impl Setup {
    async fn udp(&self, to: VisitTarget) -> Result<UdpVisit, Option<Status>> {
        udp_visit(self.tunnel.server.addr, self.visitor_secret.clone(), to).await
    }

    fn dns_status(&self) -> burrow::metrics::ServiceStatus {
        self.tunnel
            .server
            .shared
            .status()
            .services
            .into_iter()
            .find(|s| s.name == "dns")
            .unwrap()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_visitor_reaches_a_private_udp_service() {
    let s = udp_setup(|_, _| {}).await;
    let mut v = s.udp(VisitTarget::service("dns")).await.ok().unwrap();
    assert!(v.echoes(1, b"one").await);
    assert!(v.echoes(2, b"two").await);
    assert!(v.echoes(1, &pattern(60_000, 3)).await);
    let shared = s.tunnel.server.shared.clone();
    assert_eq!(shared.udp_peers(), 2);
    let status = s.dns_status();
    assert_eq!(status.total, 2, "each new peer counts once");
    assert!(status.bytes_in >= 60_006 && status.bytes_out >= 60_006);
    let laptop = shared
        .status()
        .visitors
        .into_iter()
        .find(|v| v.name == "laptop")
        .unwrap();
    assert_eq!(
        laptop.open, 1,
        "a UDP visit is one open connection of its visitor"
    );
    drop(v);
    wait_for(|| shared.udp_peers() == 0).await;
    wait_for(|| shared.status().visitors[0].open == 0).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_udp_visit_must_match_the_protocol_of_its_service() {
    let s = udp_setup(|srv, echo| {
        srv.services
            .insert("other".into(), private_udp_service(echo, &["someone"]));
    })
    .await;
    assert_eq!(
        s.ask(false, VisitTarget::service("dns")).await,
        Ok(Status::Invalid),
        "a TCP visit to a UDP service"
    );
    assert_eq!(
        s.udp(VisitTarget::service("db")).await.err(),
        Some(Some(Status::Invalid)),
        "a UDP visit to a TCP service"
    );
    assert_eq!(
        s.udp(VisitTarget::service("other")).await.err(),
        Some(Some(Status::Forbidden)),
        "a UDP service that does not list the visitor"
    );
    assert_eq!(
        s.udp(VisitTarget::service("nothing")).await.err(),
        Some(Some(Status::Forbidden))
    );
    let checked = udp_visit_with(
        s.tunnel.server.addr,
        s.visitor_secret.clone(),
        true,
        VisitTarget::service("dns"),
    )
    .await;
    assert!(checked.is_ok(), "a UDP check answers ok");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reach_carries_udp_inside_its_rules_and_never_to_a_unix_socket() {
    let s = udp_setup(|srv, _| {
        srv.visitors.get_mut("laptop").unwrap().reach =
            [("home".to_string(), narrowed(&["127.0.0.0/8"]))].into();
    })
    .await;
    let mut v = s.udp(s.reach()).await.ok().unwrap();
    assert!(v.echoes(7, b"by address").await);
    let port = s.echo.port();
    let mut named = s
        .udp(on_home(format!("localhost:{port}")))
        .await
        .ok()
        .unwrap();
    assert!(named.echoes(7, b"by name").await);
    assert_eq!(
        s.udp(on_home("10.1.2.3:53".into())).await.err(),
        Some(Some(Status::Forbidden))
    );
    assert_eq!(
        s.udp(on_home("unix:/run/app.sock".into())).await.err(),
        Some(Some(Status::Invalid))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_udp_visit_to_an_offline_client_is_reported() {
    let visitor_secret = Secret::generate();
    let mut server = server_settings(vec![Secret::generate()]);
    server
        .visitors
        .insert("laptop".into(), visitor_auth(visitor_secret.clone(), &[]));
    server.services.insert(
        "dns".into(),
        private_udp_service("127.0.0.1:9".parse().unwrap(), &["laptop"]),
    );
    let srv = start_server(server).await;
    assert_eq!(
        udp_visit(srv.addr, visitor_secret, VisitTarget::service("dns"))
            .await
            .err(),
        Some(Some(Status::Offline))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_udp_visit_counts_against_the_visitor_connection_limit() {
    let s = udp_setup(|srv, _| {
        srv.visitors.get_mut("laptop").unwrap().max_connections = Some(1);
    })
    .await;
    let mut first = s.udp(VisitTarget::service("dns")).await.ok().unwrap();
    assert!(first.echoes(1, b"held").await);
    assert_eq!(
        s.udp(VisitTarget::service("dns")).await.err(),
        Some(Some(Status::Busy))
    );
    assert_eq!(
        s.ask(false, VisitTarget::service("db")).await,
        Ok(Status::Busy)
    );
    drop(first);
    let shared = s.tunnel.server.shared.clone();
    wait_for(|| shared.status().visitors[0].open == 0).await;
    assert!(s.udp(VisitTarget::service("dns")).await.is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reload_closes_revoked_udp_visits() {
    let mut settings = None;
    let s = udp_setup(|srv, _| {
        srv.visitors.get_mut("laptop").unwrap().reach =
            [("home".to_string(), AllowList::All)].into();
        settings = Some(srv.clone());
    })
    .await;
    let settings = settings.unwrap();
    let mut second = s.udp(VisitTarget::service("dns")).await.ok().unwrap();
    let mut by_service = s.udp(VisitTarget::service("dns")).await.ok().unwrap();
    let mut by_reach = s.udp(s.reach()).await.ok().unwrap();
    for v in [&mut second, &mut by_service, &mut by_reach] {
        assert!(v.echoes(1, b"before").await);
    }
    let mut next = settings.clone();
    next.services.get_mut("dns").unwrap().visitors.clear();
    next.services
        .insert("dns2".into(), private_udp_service(s.echo, &["laptop"]));
    next.visitors.get_mut("laptop").unwrap().reach =
        [("home".to_string(), narrowed(&["10.0.0.0/8"]))].into();
    s.tunnel.server.reload(next).await;
    assert!(by_service.closed_within(Duration::from_secs(5)).await);
    assert!(by_reach.closed_within(Duration::from_secs(5)).await);
    assert!(second.closed_within(Duration::from_secs(5)).await);
    let mut back = settings;
    back.visitors.get_mut("laptop").unwrap().reach = [("home".to_string(), AllowList::All)].into();
    s.tunnel.server.reload(back.clone()).await;
    let shared = s.tunnel.server.shared.clone();
    wait_for(|| {
        shared.settings_snapshot().services["dns"]
            .visitors
            .contains(&"laptop".to_string())
    })
    .await;
    let mut open = s.udp(VisitTarget::service("dns")).await.ok().unwrap();
    assert!(open.echoes(1, b"again").await);
    back.services
        .insert("unrelated".into(), private_service(s.echo, &["laptop"]));
    s.tunnel.server.reload(back).await;
    assert!(
        open.echoes(1, b"still open").await,
        "an unrelated change keeps the visit"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_udp_visit_is_closed_by_the_server() {
    let s = udp_setup(|srv, _| {
        srv.udp_session_timeout = Duration::from_secs(1);
    })
    .await;
    let mut v = s.udp(VisitTarget::service("dns")).await.ok().unwrap();
    assert!(v.echoes(1, b"once").await);
    assert!(v.closed_within(Duration::from_secs(10)).await);
    let shared = s.tunnel.server.shared.clone();
    wait_for(|| shared.udp_peers() == 0).await;
    let mut quiet = s.udp(VisitTarget::service("dns")).await.ok().unwrap();
    assert!(
        quiet.closed_within(Duration::from_secs(10)).await,
        "a visit that never sends is closed too"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn udp_max_peers_caps_each_visit_and_keeps_known_peers() {
    let s = udp_setup(|srv, _| {
        srv.udp_max_peers = 2;
    })
    .await;
    let mut v = s.udp(VisitTarget::service("dns")).await.ok().unwrap();
    assert!(v.echoes(1, b"a").await);
    assert!(v.echoes(2, b"b").await);
    let counted = s.dns_status().bytes_in;
    assert!(!v.echoes(3, b"c").await, "a third peer is dropped");
    assert_eq!(
        s.dns_status().bytes_in,
        counted,
        "datagrams dropped at udp.max_peers are not counted as traffic"
    );
    assert!(v.echoes(1, b"a again").await);
    assert_eq!(s.tunnel.server.shared.udp_peers(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_udp_visit_record_without_a_peer_id_ends_the_visit() {
    let s = udp_setup(|_, _| {}).await;
    let mut v = s.udp(VisitTarget::service("dns")).await.ok().unwrap();
    assert!(v.echoes(1, b"fine").await);
    v.send_raw(&[0, 0, 1]).await.unwrap();
    assert!(v.closed_within(Duration::from_secs(5)).await);
    let shared = s.tunnel.server.shared.clone();
    wait_for(|| shared.udp_peers() == 0).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn udp_visits_over_the_bandwidth_limit_are_dropped() {
    let s = udp_setup(|srv, _| {
        srv.services.get_mut("dns").unwrap().bandwidth = Some(1000);
    })
    .await;
    let mut v = s.udp(VisitTarget::service("dns")).await.ok().unwrap();
    let payload = vec![5u8; 1000];
    for _ in 0..100 {
        v.send(1, &payload).await;
    }
    let mut back = 0;
    while let Got::Datagram(..) = v.next(Duration::from_secs(2)).await {
        back += 1;
    }
    assert!(back > 0 && back < 50, "{back} of 100 came back");
}

async fn grants_for(s: &Setup, request: &[u8]) -> Option<burrow::protocol::Grants> {
    let dialer = Dialer::visitor(&visitor_settings(
        s.tunnel.server.addr,
        s.visitor_secret.clone(),
        vec![],
    ));
    let mut ch = dialer
        .open(ChannelKind::Visit, Hello::Visit { version: 1 })
        .await
        .ok()?;
    ch.writer.send(&mut ch.stream, request).await.ok()?;
    let rec = ch.reader.recv(&mut ch.stream).await.ok()??.to_vec();
    burrow::protocol::Grants::decode(&rec).ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grants_list_udp_services_only_when_asked_with_protocols() {
    let s = udp_setup(|_, _| {}).await;
    let old = grants_for(&s, &burrow::protocol::GRANTS_REQUEST)
        .await
        .unwrap();
    let names = |g: &burrow::protocol::Grants| {
        g.services
            .iter()
            .map(|s| (s.name.clone(), s.protocol))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        names(&old),
        vec![("db".to_string(), burrow_config::Protocol::Tcp)]
    );
    let new = grants_for(&s, &burrow::protocol::GRANTS_REQUEST_V2)
        .await
        .unwrap();
    assert_eq!(
        names(&new),
        vec![
            ("db".to_string(), burrow_config::Protocol::Tcp),
            ("dns".to_string(), burrow_config::Protocol::Udp)
        ]
    );
    assert!(grants_for(&s, &[0x21, 0x02]).await.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_udp_visit_moves_to_the_next_client_when_one_refuses() {
    let echo = udp_echo_server().await;
    let home = Secret::generate();
    let office = Secret::generate();
    let visitor_secret = Secret::generate();
    let mut server = server_settings(vec![home.clone()]);
    server.clients.insert(
        "office".into(),
        burrow::config::settings::ClientAuth {
            secrets: vec![office.clone()],
            max_connections: None,
        },
    );
    server
        .visitors
        .insert("laptop".into(), visitor_auth(visitor_secret.clone(), &[]));
    let mut dns = private_udp_service(echo, &["laptop"]);
    dns.clients = vec!["office".into(), "home".into()];
    server.services.insert("dns".into(), dns);
    let srv = start_server(server).await;
    let mut refusing = client_settings(srv.addr, office);
    refusing.name = "office".into();
    refusing.allow = narrowed(&["10.0.0.0/8"]);
    let mut c1 = start_client(refusing).await;
    let mut c2 = start_client(client_settings(srv.addr, home)).await;
    wait_state(&mut c1.state, burrow::client::State::Connected).await;
    wait_state(&mut c2.state, burrow::client::State::Connected).await;
    let shared = srv.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 1 && shared.idle_channels("office") >= 1).await;
    let mut v = udp_visit(srv.addr, visitor_secret, VisitTarget::service("dns"))
        .await
        .ok()
        .unwrap();
    assert!(v.echoes(1, b"via home").await);
    let dns = srv
        .shared
        .status()
        .services
        .into_iter()
        .find(|s| s.name == "dns")
        .unwrap();
    assert_eq!(dns.set_aside.len(), 1, "{:?}", dns.set_aside);
    assert_eq!(dns.set_aside[0].client, "office");
    assert!(dns.set_aside[0].refused);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hand_built_v011_requests_keep_working() {
    let s = udp_setup(|_, _| {}).await;
    let send = |bytes: Vec<u8>| {
        let server = s.tunnel.server.addr;
        let secret = s.visitor_secret.clone();
        async move {
            let dialer = Dialer::visitor(&visitor_settings(server, secret, vec![]));
            let mut ch = dialer
                .open(ChannelKind::Visit, Hello::Visit { version: 1 })
                .await
                .ok()
                .unwrap();
            ch.writer.send(&mut ch.stream, &bytes).await.unwrap();
            ch.reader
                .recv(&mut ch.stream)
                .await
                .unwrap()
                .unwrap()
                .to_vec()
        }
    };
    assert_eq!(
        send(vec![0x20, 1, 1, 2, b'd', b'b', 0, 0]).await,
        vec![Status::Ok as u8]
    );
    assert_eq!(
        send(vec![0x20, 1, 1, 3, b'd', b'n', b's', 0, 0]).await,
        vec![Status::Invalid as u8],
        "a v0.1.1 visitor asking for a UDP service over TCP"
    );
    let grants = send(vec![0x21]).await;
    assert_eq!(grants[0], 0x22);
    let old = burrow::protocol::Grants::decode(&grants).unwrap();
    assert_eq!(old.services.len(), 1);
    assert_eq!(old.services[0].name, "db");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_udp_visit_closes_when_its_service_target_changes() {
    let mut settings = None;
    let s = udp_setup(|srv, _| {
        settings = Some(srv.clone());
    })
    .await;
    let mut v = s.udp(VisitTarget::service("dns")).await.ok().unwrap();
    assert!(v.echoes(1, b"before").await);
    let mut reach = settings.clone().unwrap();
    reach.visitors.get_mut("laptop").unwrap().reach = [("home".to_string(), AllowList::All)].into();
    s.tunnel.server.reload(reach.clone()).await;
    let shared = s.tunnel.server.shared.clone();
    wait_for(|| {
        shared.settings_snapshot().visitors["laptop"]
            .reach
            .contains_key("home")
    })
    .await;
    let mut by_reach = s.udp(s.reach()).await.ok().unwrap();
    assert!(v.echoes(1, b"same target").await);
    let fresh = udp_echo_server().await;
    let mut next = reach;
    next.services
        .insert("dns".into(), private_udp_service(fresh, &["laptop"]));
    s.tunnel.server.reload(next).await;
    assert!(
        v.closed_within(Duration::from_secs(5)).await,
        "a UDP visit to a service whose target changed is closed"
    );
    assert!(
        by_reach.echoes(1, b"reach is unaffected").await,
        "a reach visit keeps its own target"
    );
    let mut again = s.udp(VisitTarget::service("dns")).await.ok().unwrap();
    assert!(again.echoes(1, b"after").await);
    let got = s
        .tunnel
        .server
        .shared
        .status()
        .services
        .into_iter()
        .find(|x| x.name == "dns")
        .unwrap()
        .target;
    assert_eq!(got, fresh.to_string());
}

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
