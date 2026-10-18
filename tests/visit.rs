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
            &VisitRequest { check, to }.encode().unwrap(),
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
        let mut web = service(echo, burrow::config::model::Protocol::Tcp);
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
    let mut open = open_echo(v.addr("echo")).await;
    next.visitors.get_mut("laptop").unwrap().reach =
        [("home".to_string(), narrowed(&["10.0.0.0/8"]))].into();
    s.tunnel.server.reload(next).await;
    let mut buf = [0u8; 4];
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        let _ = open.write_all(b"ping").await;
        matches!(open.read(&mut buf).await, Ok(0) | Err(_))
    })
    .await;
    assert_eq!(
        closed,
        Ok(true),
        "the open connection outside the new reach stayed open"
    );
    assert!(refused(v.addr("echo")).await);
}
