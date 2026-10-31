mod common;
#[path = "failover/entries.rs"]
mod entries;
#[path = "failover/health.rs"]
mod health;
#[cfg(feature = "web")]
#[path = "failover/http_check.rs"]
mod http_check;
#[cfg(feature = "web")]
#[path = "failover/notify.rs"]
mod notify;
#[path = "failover/selection.rs"]
mod selection;
#[path = "failover/udp_moves.rs"]
mod udp_moves;
#[path = "failover/weights.rs"]
mod weights;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use burrow::client::{Client, State};
use burrow::config::settings::{ClientAuth, Health, ServerSettings, Service};
use burrow::policy::allowlist::AllowList;
use burrow::wire::secret::Secret;
use burrow::wire::transport::resolve::{Resolved, Resolver};
use burrow_config::{Mode, Protocol};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use common::*;
use health::{tagged_udp_echo, udp_tag};

const TARGET: &str = "app.test:80";

struct Local(SocketAddr);

impl Resolver for Local {
    fn resolve<'a>(&'a self, host_port: &'a str) -> Resolved<'a> {
        Box::pin(async move {
            if host_port == TARGET {
                Ok(vec![self.0])
            } else {
                burrow::wire::transport::resolve::resolve(host_port).await
            }
        })
    }
}

struct OnceElsewhere(SocketAddr, std::sync::atomic::AtomicBool);

impl Resolver for OnceElsewhere {
    fn resolve<'a>(&'a self, host_port: &'a str) -> Resolved<'a> {
        Box::pin(async move {
            if host_port != TARGET {
                return burrow::wire::transport::resolve::resolve(host_port).await;
            }
            if self.1.swap(false, std::sync::atomic::Ordering::AcqRel) {
                return Ok(vec![SocketAddr::new([10, 0, 0, 1].into(), self.0.port())]);
            }
            Ok(vec![self.0])
        })
    }
}

struct Slow(SocketAddr);

impl Resolver for Slow {
    fn resolve<'a>(&'a self, host_port: &'a str) -> Resolved<'a> {
        Box::pin(async move {
            if host_port != TARGET {
                return burrow::wire::transport::resolve::resolve(host_port).await;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok(vec![self.0])
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
    plaintext: bool,
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
        let plaintext = settings.insecure_plaintext;
        let server = start_server(settings).await;
        let mut farm = Farm {
            server,
            sites,
            plaintext,
        };
        for i in 0..farm.sites.len() {
            farm.connect(i).await;
        }
        farm
    }

    async fn connect(&mut self, i: usize) {
        self.connect_allowing(i, "127.0.0.0/8").await;
    }

    async fn connect_refusing(&mut self, i: usize) {
        self.disconnect(i).await;
        self.connect_allowing(i, "10.0.0.0/8").await;
    }

    async fn connect_allowing(&mut self, i: usize, allow: &str) {
        let target = self.sites[i].target;
        self.connect_using(i, allow, Arc::new(Local(target))).await;
    }

    async fn connect_slow(&mut self, i: usize) {
        self.disconnect(i).await;
        let target = self.sites[i].target;
        self.connect_with(i, "127.0.0.0/8", Arc::new(Slow(target)), |c| {
            c.connect_timeout = Duration::from_secs(30)
        })
        .await;
    }

    async fn connect_using(&mut self, i: usize, allow: &str, resolver: Arc<dyn Resolver>) {
        self.connect_with(i, allow, resolver, |_| {}).await;
    }

    async fn connect_with(
        &mut self,
        i: usize,
        allow: &str,
        resolver: Arc<dyn Resolver>,
        tweak: impl FnOnce(&mut burrow::config::settings::ClientSettings),
    ) {
        let site = &self.sites[i];
        let mut c = client_settings(self.server.addr, site.secret.clone());
        c.name = site.name.into();
        c.allow = AllowList::Rules(vec![allow.parse().unwrap()]);
        c.insecure_plaintext = self.plaintext;
        tweak(&mut c);
        let client = Client::new(over_transport(c)).with_resolver(resolver);
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
    let reached = within_tries(100, || async move { served_by(addr).await == Some(want) }).await;
    assert!(reached, "connections never reached client {want}");
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
        http: None,
    })
}
