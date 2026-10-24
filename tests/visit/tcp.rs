use std::time::Duration;

use burrow::config::settings::ServerSettings;
use burrow::dialer::{DialError, Dialer};
use burrow::policy::allowlist::{AllowList, Rule};
use burrow::wire::protocol::{ChannelKind, Hello, Status, VisitTarget};
use burrow::wire::secret::Secret;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::common::*;
use super::{ask, refused, setup, setup_with};

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
        srv.visitors.get_mut("laptop").unwrap().reach = [(
            "home".to_string(),
            burrow::policy::allowlist::AllowList::All,
        )]
        .into();
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
            srv.visitors.get_mut("laptop").unwrap().reach = [(
                "home".to_string(),
                burrow::policy::allowlist::AllowList::All,
            )]
            .into();
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
        srv.visitors.get_mut("laptop").unwrap().reach = [(
            "home".to_string(),
            burrow::policy::allowlist::AllowList::All,
        )]
        .into();
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
        srv.visitors.get_mut("laptop").unwrap().reach = [(
            "home".to_string(),
            burrow::policy::allowlist::AllowList::All,
        )]
        .into();
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
