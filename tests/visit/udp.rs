use std::net::SocketAddr;
use std::time::Duration;

use burrow::config::settings::ServerSettings;
use burrow::dialer::Dialer;
use burrow::policy::allowlist::AllowList;
use burrow::wire::protocol::{ChannelKind, Hello, Status, VisitTarget};
use burrow::wire::secret::Secret;

use super::Setup;
use super::common::*;
use super::requests::{narrowed, on_home};

pub(super) async fn udp_setup(change: impl FnOnce(&mut ServerSettings, SocketAddr)) -> Setup {
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
    pub(super) async fn udp(&self, to: VisitTarget) -> Result<UdpVisit, Option<Status>> {
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

async fn grants_for(s: &Setup, request: &[u8]) -> Option<burrow::wire::protocol::Grants> {
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
    burrow::wire::protocol::Grants::decode(&rec).ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grants_list_udp_services_only_when_asked_with_protocols() {
    let s = udp_setup(|_, _| {}).await;
    let old = grants_for(&s, &burrow::wire::protocol::GRANTS_REQUEST)
        .await
        .unwrap();
    let names = |g: &burrow::wire::protocol::Grants| {
        g.services
            .iter()
            .map(|s| (s.name.clone(), s.protocol))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        names(&old),
        vec![("db".to_string(), burrow_config::Protocol::Tcp)]
    );
    let new = grants_for(&s, &burrow::wire::protocol::GRANTS_REQUEST_V2)
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
    let old = burrow::wire::protocol::Grants::decode(&grants).unwrap();
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
