use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::Duration;

use burrow_config::Mode;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

use super::common::*;
use super::{Farm, eventually_served_by, quick_health, served_by, web};

async fn held(addr: SocketAddr) -> (TcpStream, char) {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let mut tag = [0u8; 1];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut tag))
        .await
        .expect("a tag in time")
        .expect("a tag");
    (s, tag[0] as char)
}

fn open_of(farm: &Farm, client: &str) -> u64 {
    farm.server
        .shared
        .status()
        .clients
        .iter()
        .find(|c| c.name == client)
        .map_or(0, |c| c.open)
}

async fn settled(farm: &Farm, a: u64, b: u64) {
    wait_for(|| open_of(farm, "a") == a && open_of(farm, "b") == b).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn balance_gives_each_client_its_weight() {
    let mut svc = web(Mode::Balance, None);
    svc.weights.insert("a".into(), 3);
    let farm = Farm::start(svc, |_| {}).await;
    let addr = farm.server.services["web"];
    let mut seen = BTreeMap::new();
    for _ in 0..40 {
        *seen.entry(served_by(addr).await.unwrap()).or_insert(0) += 1;
    }
    let a = seen.get(&'A').copied().unwrap_or(0);
    assert!((28..=32).contains(&a), "{seen:?}");
    assert_eq!(seen.values().sum::<i32>(), 40, "{seen:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn least_connections_picks_the_idle_client() {
    let farm = Farm::start(web(Mode::LeastConnections, None), |_| {}).await;
    let addr = farm.server.services["web"];
    let mut open = Vec::new();
    for (want, a, b) in [
        ('A', 1, 0),
        ('B', 1, 1),
        ('A', 2, 1),
        ('B', 2, 2),
        ('A', 3, 2),
    ] {
        let (s, by) = held(addr).await;
        assert_eq!(
            by, want,
            "fewest open connections first, a tie in list order"
        );
        open.push((s, by));
        settled(&farm, a, b).await;
    }
    open.retain(|(_, by)| *by == 'A');
    settled(&farm, 3, 0).await;
    for _ in 0..3 {
        assert_eq!(served_by(addr).await, Some('B'), "a holds 3, b is idle");
        settled(&farm, 3, 0).await;
    }
    drop(open);
    settled(&farm, 0, 0).await;
    assert_eq!(served_by(addr).await, Some('A'));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_quick_connections_go_to_two_idle_clients() {
    let farm = Farm::start(web(Mode::LeastConnections, None), |_| {}).await;
    let addr = farm.server.services["web"];
    for _ in 0..5 {
        let ((one, x), (two, y)) = tokio::join!(held(addr), held(addr));
        let mut got = [x, y];
        got.sort();
        assert_eq!(
            got,
            ['A', 'B'],
            "the count is taken when a client is picked"
        );
        drop((one, two));
        settled(&farm, 0, 0).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unhealthy_client_is_skipped_by_balance_and_least_connections() {
    for mode in [Mode::Balance, Mode::LeastConnections] {
        let mut svc = web(mode, quick_health());
        if mode == Mode::Balance {
            svc.weights.insert("a".into(), 5);
        }
        let mut farm = Farm::start(svc, |_| {}).await;
        let addr = farm.server.services["web"];
        eventually_served_by(addr, 'A').await;
        farm.stop_target(0).await;
        eventually_served_by(addr, 'B').await;
        for _ in 0..6 {
            assert_eq!(served_by(addr).await, Some('B'), "{mode:?}");
            settled(&farm, 0, 0).await;
        }
        farm.restart_target(0).await;
        eventually_served_by(addr, 'A').await;
    }
}
