use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;

use burrow::config::settings::{ClientAuth, Entry, Health, Service};
use burrow::wire::secret::Secret;
use burrow::wire::transport::resolve::{Resolved, Resolver};
use burrow_config::{Mode, Protocol};
use tokio::task::JoinHandle;

use super::common::*;
use super::weights::held;
use super::{served_by, tagged_echo_on};

const VM1: &str = "vm1.test:80";
const VM2: &str = "vm2.test:80";

struct TwoVms(SocketAddr, SocketAddr);

impl Resolver for TwoVms {
    fn resolve<'a>(&'a self, host_port: &'a str) -> Resolved<'a> {
        Box::pin(async move {
            match host_port {
                VM1 => Ok(vec![self.0]),
                VM2 => Ok(vec![self.1]),
                other => burrow::wire::transport::resolve::resolve(other).await,
            }
        })
    }
}

struct Gateway {
    server: RunningServer,
    _client: RunningClient,
    vms: [(SocketAddr, JoinHandle<()>); 2],
}

impl Gateway {
    async fn start(mode: Mode, health: Option<Health>, weights: [u32; 2]) -> Gateway {
        logs();
        let vm1 = tagged_echo_on("127.0.0.1:0".parse().unwrap(), b'A').await;
        let vm2 = tagged_echo_on("127.0.0.1:0".parse().unwrap(), b'B').await;
        let secret = Secret::generate();
        let mut settings = server_settings(vec![]);
        settings.clients = BTreeMap::from([(
            "gw".to_string(),
            ClientAuth {
                secrets: vec![secret.clone()],
                max_connections: None,
            },
        )]);
        let entry = |target: Option<&str>, weight| {
            Entry::new("gw", target.map(str::to_string), weight, VM1)
        };
        let web = Service {
            clients: vec!["gw".into()],
            entries: vec![entry(None, weights[0]), entry(Some(VM2), weights[1])],
            mode,
            health,
            target: VM1.into(),
            ..service("127.0.0.1:1".parse().unwrap(), Protocol::Tcp)
        };
        settings.services.insert("web".into(), web);
        let server = start_server(settings).await;
        let mut c = client_settings(server.addr, secret);
        c.name = "gw".into();
        let client = burrow::client::Client::new(over_transport(c))
            .with_resolver(Arc::new(TwoVms(vm1.0, vm2.0)));
        let mut running = start_client_with(client).await;
        wait_state(&mut running.state, burrow::client::State::Connected).await;
        Gateway {
            server,
            _client: running,
            vms: [vm1, vm2],
        }
    }

    fn addr(&self) -> SocketAddr {
        self.server.services["web"]
    }

    async fn stop(&mut self, i: usize) {
        self.vms[i].1.abort();
        let _ = (&mut self.vms[i].1).await;
    }

    async fn restart(&mut self, i: usize) {
        let tag = if i == 0 { b'A' } else { b'B' };
        self.vms[i] = tagged_echo_on(self.vms[i].0, tag).await;
    }

    async fn spread(&self, n: usize) -> BTreeMap<char, usize> {
        let mut seen = BTreeMap::new();
        for _ in 0..n {
            *seen
                .entry(served_by(self.addr()).await.unwrap_or('-'))
                .or_insert(0) += 1;
            self.settled(0).await;
        }
        seen
    }

    async fn settled(&self, open: u64) {
        let shared = self.server.shared.clone();
        wait_for(|| {
            shared.status().services[0]
                .entries
                .iter()
                .map(|e| e.open)
                .sum::<u64>()
                == open
        })
        .await;
    }
}

async fn eventually(g: &Gateway, want: char) {
    let addr = g.addr();
    let reached = within_tries(100, || async move { served_by(addr).await == Some(want) }).await;
    assert!(reached, "connections never reached VM {want}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_vms_behind_one_gateway_take_turns_and_one_that_is_down_is_skipped() {
    let mut g = Gateway::start(Mode::Balance, Some(quick_health()), [1, 1]).await;
    let seen = g.spread(20).await;
    assert_eq!(
        (seen.get(&'A'), seen.get(&'B')),
        (Some(&10), Some(&10)),
        "{seen:?}"
    );
    g.stop(0).await;
    eventually(&g, 'B').await;
    let shared = g.server.shared.clone();
    wait_for(|| {
        let s = shared.status();
        let h = &s.services[0].health;
        h.len() == 2
            && h.iter().any(|h| h.target.is_none() && !h.up)
            && h.iter().any(|h| h.target.as_deref() == Some(VM2) && h.up)
    })
    .await;
    let seen = g.spread(6).await;
    assert_eq!(
        seen.get(&'B'),
        Some(&6),
        "the gateway's other VM takes everything: {seen:?}"
    );
    g.restart(0).await;
    eventually(&g, 'A').await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failover_uses_the_first_entry_of_the_gateway() {
    let mut g = Gateway::start(Mode::Failover, None, [1, 1]).await;
    assert_eq!(g.spread(5).await.get(&'A'), Some(&5));
    g.stop(0).await;
    let seen = g.spread(5).await;
    assert_eq!(
        seen.get(&'B'),
        Some(&5),
        "a VM that refuses is set aside for that entry only: {seen:?}"
    );
    let s = g.server.shared.status();
    assert_eq!(
        s.services[0].set_aside.len(),
        1,
        "{:?}",
        s.services[0].set_aside
    );
    assert_eq!(s.services[0].set_aside[0].target, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn weights_and_least_connections_count_each_entry() {
    let g = Gateway::start(Mode::Balance, None, [3, 1]).await;
    let a = g.spread(40).await.get(&'A').copied().unwrap_or(0);
    assert!(
        (28..=32).contains(&a),
        "{a} of 40 went to the weight-3 entry"
    );
    let g = Gateway::start(Mode::LeastConnections, None, [1, 1]).await;
    let mut open = Vec::new();
    for (want, n) in [('A', 1), ('B', 2), ('A', 3), ('B', 4)] {
        let (s, by) = held(g.addr()).await;
        assert_eq!(
            by, want,
            "the entry with fewer open connections, a tie in list order"
        );
        open.push(s);
        g.settled(n).await;
    }
    let ((one, x), (two, y)) = tokio::join!(held(g.addr()), held(g.addr()));
    let mut got = [x, y];
    got.sort();
    assert_eq!(
        got,
        ['A', 'B'],
        "two quick connections go to the two entries"
    );
    drop((one, two, open));
}
