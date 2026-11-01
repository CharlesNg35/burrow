use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use burrow::config::settings::Service;
use burrow::metrics::SetAside;
use burrow::wire::protocol::VisitTarget;
use burrow::wire::secret::Secret;
use burrow_config::{Mode, Protocol};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::common::*;
use super::{
    Farm, OnceElsewhere, TARGET, eventually_served_by, quick_health, served_by, tagged_udp_echo,
    udp_tag, web,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failover_uses_the_first_client_and_moves_while_it_is_away() {
    let mut farm = Farm::start(web(Mode::Failover, None), |_| {}).await;
    let addr = farm.server.services["web"];
    for _ in 0..5 {
        assert_eq!(served_by(addr).await, Some('A'));
    }
    let open = TcpStream::connect(addr).await.unwrap();
    farm.disconnect(0).await;
    for _ in 0..5 {
        assert_eq!(served_by(addr).await, Some('B'));
    }
    farm.connect(0).await;
    eventually_served_by(addr, 'A').await;
    drop(open);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn balance_spreads_new_connections_over_the_clients() {
    let farm = Farm::start(web(Mode::Balance, None), |_| {}).await;
    let addr = farm.server.services["web"];
    let mut seen = BTreeMap::new();
    for _ in 0..20 {
        *seen.entry(served_by(addr).await.unwrap()).or_insert(0) += 1;
    }
    assert_eq!(seen.get(&'A'), Some(&10), "{seen:?}");
    assert_eq!(seen.get(&'B'), Some(&10), "{seen:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn health_checks_skip_a_client_whose_target_is_down() {
    let mut farm = Farm::start(web(Mode::Failover, quick_health()), |_| {}).await;
    let addr = farm.server.services["web"];
    eventually_served_by(addr, 'A').await;
    farm.stop_target(0).await;
    eventually_served_by(addr, 'B').await;
    for _ in 0..5 {
        assert_eq!(served_by(addr).await, Some('B'));
    }
    farm.restart_target(0).await;
    eventually_served_by(addr, 'A').await;
}

fn set_aside(farm: &Farm) -> Vec<SetAside> {
    farm.server.shared.status().services[0].set_aside.clone()
}

pub(super) fn aside(client: &str, refused: bool) -> SetAside {
    SetAside {
        client: client.into(),
        target: None,
        refused,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_down_target_is_skipped_for_each_connection() {
    let mut farm = Farm::start(web(Mode::Failover, None), |_| {}).await;
    let addr = farm.server.services["web"];
    farm.stop_target(0).await;
    for _ in 0..5 {
        assert_eq!(served_by(addr).await, Some('B'));
    }
    assert_eq!(set_aside(&farm), vec![aside("a", false)]);
    farm.stop_target(1).await;
    assert_eq!(served_by(addr).await, None);
    farm.restart_target(0).await;
    assert_eq!(
        served_by(addr).await,
        Some('A'),
        "a client set aside is still tried when the others fail"
    );
    assert_eq!(set_aside(&farm), vec![aside("b", false)]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_that_refuses_the_target_is_skipped_until_it_reconnects() {
    for mode in [Mode::Failover, Mode::Balance] {
        let mut farm = Farm::start(web(mode, None), |_| {}).await;
        let addr = farm.server.services["web"];
        farm.connect_refusing(0).await;
        for _ in 0..10 {
            assert_eq!(served_by(addr).await, Some('B'), "{mode:?}");
        }
        assert_eq!(set_aside(&farm), vec![aside("a", true)]);
        farm.disconnect(1).await;
        assert_eq!(
            served_by(addr).await,
            None,
            "with only a refusing client left, connections fail as before"
        );
        farm.connect(1).await;
        farm.disconnect(0).await;
        farm.connect(0).await;
        assert!(set_aside(&farm).is_empty());
        let mut seen = BTreeMap::new();
        for _ in 0..10 {
            *seen.entry(served_by(addr).await.unwrap()).or_insert(0) += 1;
        }
        let want = match mode {
            Mode::Failover => BTreeMap::from([('A', 10)]),
            Mode::Balance => BTreeMap::from([('A', 5), ('B', 5)]),
            Mode::LeastConnections | Mode::IpHash => unreachable!("not one of the modes above"),
        };
        assert_eq!(seen, want, "{mode:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_that_refused_once_is_still_tried_when_the_others_fail() {
    let mut farm = Farm::start(web(Mode::Failover, None), |_| {}).await;
    let addr = farm.server.services["web"];
    farm.disconnect(0).await;
    let target = farm.sites[0].target;
    let flaky = Arc::new(OnceElsewhere(
        target,
        std::sync::atomic::AtomicBool::new(true),
    ));
    farm.connect_using(0, "127.0.0.0/8", flaky).await;
    assert_eq!(served_by(addr).await, Some('B'), "the one refusal moves on");
    assert_eq!(set_aside(&farm), vec![aside("a", true)]);
    farm.stop_target(1).await;
    assert_eq!(
        served_by(addr).await,
        Some('A'),
        "a refusing client is still tried last, and serves once it can"
    );
    assert!(set_aside(&farm).iter().all(|s| s.client != "a"));
}

async fn echoed_through(addr: SocketAddr, data: Vec<u8>) -> Vec<u8> {
    let s = TcpStream::connect(addr).await.unwrap();
    let (mut r, mut w) = s.into_split();
    let writer = tokio::spawn(async move {
        w.write_all(&data).await.unwrap();
        w.shutdown().await.unwrap();
    });
    let mut back = Vec::new();
    tokio::time::timeout(Duration::from_secs(20), r.read_to_end(&mut back))
        .await
        .unwrap()
        .unwrap();
    writer.await.unwrap();
    back
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn what_the_visitor_sent_first_reaches_the_next_client_intact() {
    for plaintext in [false, true] {
        let mut farm = Farm::start(web(Mode::Failover, None), |s| {
            s.insecure_plaintext = plaintext;
        })
        .await;
        let addr = farm.server.services["web"];
        for refusing in [true, false] {
            if refusing {
                farm.connect_refusing(0).await;
            } else {
                farm.disconnect(0).await;
                farm.connect(0).await;
                farm.stop_target(0).await;
            }
            for size in [0usize, 10, 70_000, 1_000_000] {
                let data: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
                let back = echoed_through(addr, data.clone()).await;
                assert_eq!(
                    back.first(),
                    Some(&b'B'),
                    "plaintext {plaintext}, {size} bytes"
                );
                assert!(
                    back[1..] == data[..],
                    "plaintext {plaintext}, refusing {refusing}: {size} bytes sent, {} came back",
                    back.len() - 1
                );
            }
        }
        let shared = farm.server.shared.clone();
        wait_for(|| shared.active_connections() == 0).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bytes_a_client_got_before_it_went_silent_are_never_sent_to_another() {
    let mut farm = Farm::start(web(Mode::Failover, None), |_| {}).await;
    let addr = farm.server.services["web"];
    farm.connect_slow(0).await;
    let back = echoed_through(addr, b"POST /pay".to_vec()).await;
    assert!(
        back.is_empty(),
        "the bytes may already be on their way to the target through client a: {back:?}"
    );
    assert_eq!(set_aside(&farm), vec![aside("a", false)]);
    assert_eq!(served_by(addr).await, Some('B'));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_visitor_that_sent_nothing_yet_moves_on_from_a_silent_client() {
    let mut farm = Farm::start(web(Mode::Failover, None), |_| {}).await;
    let addr = farm.server.services["web"];
    farm.connect_slow(0).await;
    assert_eq!(served_by(addr).await, Some('B'));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refusing_client_is_skipped_for_visitors_too() {
    let visitor_secret = Secret::generate();
    let private = Service {
        bind: None,
        visitors: vec!["laptop".into()],
        ..web(Mode::Failover, None)
    };
    let mut farm = Farm::start(private, |s| {
        s.visitors
            .insert("laptop".into(), visitor_auth(visitor_secret.clone(), &[]));
    })
    .await;
    farm.connect_refusing(0).await;
    let v = start_visitor(visitor_settings(
        farm.server.addr,
        visitor_secret,
        vec![("web", VisitTarget::service("web"))],
    ));
    for _ in 0..5 {
        assert_eq!(served_by(v.addr("web")).await, Some('B'));
    }
    let data: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
    let back = echoed_through(v.addr("web"), data.clone()).await;
    assert_eq!(back.first(), Some(&b'B'));
    assert!(back[1..] == data[..], "{} bytes came back", back.len() - 1);
    assert_eq!(set_aside(&farm), vec![aside("a", true)]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_udp_service_moves_away_from_a_refusing_client() {
    let ta = tagged_udp_echo(b'A').await;
    let dns = Service {
        clients: vec!["a".into(), "b".into()],
        target: TARGET.into(),
        ..service(ta, Protocol::Udp)
    };
    let mut farm = Farm::start(dns, |_| {}).await;
    for (i, tag) in [(0, b'A'), (1, b'B')] {
        farm.disconnect(i).await;
        farm.sites[i].target = tagged_udp_echo(tag).await;
        farm.connect(i).await;
    }
    let s = udp_bind().await;
    s.connect(farm.server.services["web"]).await.unwrap();
    assert_eq!(udp_tag(&s).await, Some(b'A'));
    farm.connect_refusing(0).await;
    let mut tag = None;
    for _ in 0..20 {
        tag = udp_tag(&s).await;
        if tag == Some(b'B') {
            break;
        }
    }
    assert_eq!(tag, Some(b'B'));
    let shared = farm.server.shared.clone();
    wait_for(|| shared.status().services[0].set_aside == vec![aside("a", true)]).await;
    assert_eq!(
        udp_tag(&s).await,
        Some(b'B'),
        "the relay does not move back to the refusing client"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn visitors_fail_over_too() {
    let visitor_secret = Secret::generate();
    let private = Service {
        bind: None,
        visitors: vec!["laptop".into()],
        ..web(Mode::Failover, None)
    };
    let mut farm = Farm::start(private, |s| {
        s.visitors
            .insert("laptop".into(), visitor_auth(visitor_secret.clone(), &[]));
    })
    .await;
    let v = start_visitor(visitor_settings(
        farm.server.addr,
        visitor_secret,
        vec![("web", VisitTarget::service("web"))],
    ));
    assert_eq!(served_by(v.addr("web")).await, Some('A'));
    farm.disconnect(0).await;
    assert_eq!(served_by(v.addr("web")).await, Some('B'));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_udp_service_uses_whichever_client_is_connected() {
    let echo = udp_echo_server().await;
    let dns = Service {
        clients: vec!["a".into(), "b".into()],
        target: echo.to_string(),
        ..service(echo, Protocol::Udp)
    };
    let mut farm = Farm::start(dns, |_| {}).await;
    farm.disconnect(0).await;
    let s = udp_bind().await;
    s.connect(farm.server.services["web"]).await.unwrap();
    let mut buf = [0u8; 64];
    for _ in 0..50 {
        s.send(b"hello").await.unwrap();
        if let Ok(Ok(n)) = tokio::time::timeout(Duration::from_millis(200), s.recv(&mut buf)).await
            && &buf[..n] == b"hello"
        {
            return;
        }
    }
    panic!("the UDP service must work through the remaining client");
}
