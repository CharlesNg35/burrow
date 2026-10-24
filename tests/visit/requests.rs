use std::time::Duration;

use burrow::dialer::Dialer;
use burrow::policy::allowlist::{AllowList, Rule};
use burrow::wire::protocol::{ChannelKind, Hello, Status, VisitTarget};
use burrow::wire::secret::Secret;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::common::*;
use super::{ask, refused, setup, setup_with};

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
        srv.visitors.get_mut("laptop").unwrap().reach = [(
            "home".to_string(),
            burrow::policy::allowlist::AllowList::All,
        )]
        .into();
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

impl burrow::wire::transport::resolve::Resolver for SlowName {
    fn resolve<'a>(&'a self, host_port: &'a str) -> burrow::wire::transport::resolve::Resolved<'a> {
        Box::pin(async move {
            if host_port.starts_with("slow.test:") {
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            burrow::wire::transport::resolve::resolve(host_port).await
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

pub(super) fn narrowed(rules: &[&str]) -> AllowList {
    AllowList::Rules(rules.iter().map(|r| r.parse::<Rule>().unwrap()).collect())
}

pub(super) fn on_home(target: String) -> VisitTarget {
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
