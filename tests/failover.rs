mod common;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use burrow::client::{Client, State};
use burrow::config::model::{Mode, Protocol};
use burrow::config::settings::{ClientAuth, Health, ServerSettings, Service};
use burrow::protocol::VisitTarget;
use burrow::secret::Secret;
use burrow::transport::resolve::{Resolved, Resolver};
use common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::task::JoinHandle;

const TARGET: &str = "app.test:80";

struct Local(SocketAddr);

impl Resolver for Local {
    fn resolve<'a>(&'a self, host_port: &'a str) -> Resolved<'a> {
        Box::pin(async move {
            if host_port == TARGET {
                Ok(vec![self.0])
            } else {
                burrow::transport::resolve::resolve(host_port).await
            }
        })
    }
}

async fn tagged_echo_on(addr: SocketAddr, tag: u8) -> (SocketAddr, JoinHandle<()>) {
    let l = TcpListener::bind(addr).await.unwrap();
    let addr = l.local_addr().unwrap();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                return;
            };
            tokio::spawn(async move {
                if s.write_all(&[tag]).await.is_err() {
                    return;
                }
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    (addr, task)
}

struct Site {
    name: &'static str,
    secret: Secret,
    target: SocketAddr,
    echo: JoinHandle<()>,
    client: Option<RunningClient>,
}

struct Farm {
    server: RunningServer,
    sites: Vec<Site>,
}

impl Farm {
    async fn start(service: Service, extra: impl FnOnce(&mut ServerSettings)) -> Farm {
        logs();
        let mut sites = Vec::new();
        for (name, tag) in [("a", b'A'), ("b", b'B')] {
            let (target, echo) = tagged_echo_on("127.0.0.1:0".parse().unwrap(), tag).await;
            sites.push(Site {
                name,
                secret: Secret::generate(),
                target,
                echo,
                client: None,
            });
        }
        let mut settings = server_settings(vec![]);
        settings.clients = sites
            .iter()
            .map(|s| {
                (
                    s.name.to_string(),
                    ClientAuth {
                        secrets: vec![s.secret.clone()],
                        max_connections: None,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        settings.services.insert("web".into(), service);
        extra(&mut settings);
        let server = start_server(settings).await;
        let mut farm = Farm { server, sites };
        for i in 0..farm.sites.len() {
            farm.connect(i).await;
        }
        farm
    }

    async fn connect(&mut self, i: usize) {
        let site = &self.sites[i];
        let mut c = client_settings(self.server.addr, site.secret.clone());
        c.name = site.name.into();
        let client = Client::new(over_transport(c)).with_resolver(Arc::new(Local(site.target)));
        let mut running = start_client_with(client).await;
        wait_state(&mut running.state, State::Connected).await;
        let shared = self.server.shared.clone();
        let name = site.name;
        wait_for(|| shared.idle_channels(name) >= 1).await;
        self.sites[i].client = Some(running);
    }

    async fn disconnect(&mut self, i: usize) {
        if let Some(c) = self.sites[i].client.take() {
            c.stop.send(true).unwrap();
            c.task.await.unwrap();
        }
        let shared = self.server.shared.clone();
        let name = self.sites[i].name.to_string();
        wait_for(|| !shared.session_names().contains(&name)).await;
    }

    async fn stop_target(&mut self, i: usize) {
        self.sites[i].echo.abort();
        let _ = (&mut self.sites[i].echo).await;
    }

    async fn restart_target(&mut self, i: usize) {
        let tag = if i == 0 { b'A' } else { b'B' };
        let (_, echo) = tagged_echo_on(self.sites[i].target, tag).await;
        self.sites[i].echo = echo;
    }
}

async fn served_by(addr: SocketAddr) -> Option<char> {
    let mut s = TcpStream::connect(addr).await.ok()?;
    let mut tag = [0u8; 1];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut tag))
        .await
        .ok()?
        .ok()?;
    s.write_all(b"ping").await.ok()?;
    let mut back = [0u8; 4];
    s.read_exact(&mut back).await.ok()?;
    (&back == b"ping").then_some(tag[0] as char)
}

async fn eventually_served_by(addr: SocketAddr, want: char) {
    for _ in 0..100 {
        if served_by(addr).await == Some(want) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("connections never reached client {want}");
}

fn web(mode: Mode, health: Option<Health>) -> Service {
    Service {
        clients: vec!["a".into(), "b".into()],
        mode,
        health,
        target: TARGET.into(),
        ..service("127.0.0.1:1".parse().unwrap(), Protocol::Tcp)
    }
}

fn quick_health() -> Option<Health> {
    Some(Health {
        interval: Duration::from_millis(300),
        timeout: Duration::from_secs(1),
        failures: 1,
    })
}

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_health_checks_a_down_target_is_still_tried() {
    let mut farm = Farm::start(web(Mode::Failover, None), |_| {}).await;
    let addr = farm.server.services["web"];
    farm.stop_target(0).await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(served_by(addr).await, None);
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
    let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
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
