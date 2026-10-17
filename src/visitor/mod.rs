mod forward;
mod socks;

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::sync::{Semaphore, mpsc, watch};
use tokio::task::JoinSet;
use tracing::info;

use crate::config::check::ports::show_addr;
use crate::config::model::Protocol;
use crate::config::settings::{Forward, VisitorSettings};
use crate::dialer::Dialer;
use crate::failure::Failure;
use crate::protocol::VisitTarget;
use crate::transport::resolve::Resolver;
use crate::transport::tcp::bind_tcp;

const MAX_OPENING: usize = 32;

pub enum Pick {
    All,
    Forward {
        name: String,
        local: Option<(SocketAddr, u16)>,
    },
    Adhoc {
        to: VisitTarget,
        local: (SocketAddr, u16),
    },
    Socks {
        client: String,
        local: SocketAddr,
    },
}

pub fn pick(settings: &mut VisitorSettings, choice: Pick, config: &str) -> Result<(), Failure> {
    match choice {
        Pick::All => {
            if settings.forwards.is_empty() {
                return Err(Failure::usage(format!("{config} has no forwards")).hint(
                    format!(
                        "add one under \"forwards\", for example db: {{ service: db, local: 5432 }}, or name the target: {}, or: {}",
                        crate::failure::cmd("visit --service NAME --local PORT"),
                        crate::failure::cmd("visit --client NAME --target HOST:PORT --local PORT")
                    ),
                ));
            }
        }
        Pick::Forward { name, local } => {
            let Some(mut forward) = settings.forwards.remove(&name) else {
                let names: Vec<&str> = settings.forwards.keys().map(String::as_str).collect();
                let defined = if names.is_empty() {
                    format!("{config} has no forwards")
                } else {
                    format!("defined forwards: {}", names.join(", "))
                };
                let hint = match crate::suggest::closest(&name, names.iter().copied()) {
                    Some(s) => format!("did you mean \"{s}\"? {defined}"),
                    None => format!(
                        "{defined}; to reach a service that is not in the config, pass --service {name} --local PORT"
                    ),
                };
                return Err(
                    Failure::usage(format!("no forward named \"{name}\" in {config}")).hint(hint),
                );
            };
            if let Some((local, ports)) = local {
                if ports != forward.ports {
                    return Err(Failure::usage(format!(
                        "forward \"{name}\" has {} {}, but --local has {ports}",
                        forward.ports,
                        crate::diagnostics::plural(forward.ports.into(), "port")
                    ))
                    .hint("give --local a range of the same size"));
                }
                forward.local = local;
            }
            settings.forwards = [(name, forward)].into();
        }
        Pick::Socks { client, local } => {
            settings.forwards = [(client.clone(), Forward::socks(local, client))].into();
        }
        Pick::Adhoc { to, local } => {
            let name = match &to {
                VisitTarget::Service { name, .. } => name.clone(),
                VisitTarget::Client { client, .. } => client.clone(),
            };
            let (local, ports) = local;
            settings.forwards = [(
                name,
                Forward {
                    local,
                    ports,
                    to,
                    socks: false,
                },
            )]
            .into();
        }
    }
    Ok(())
}

struct Bound {
    name: String,
    to: VisitTarget,
    socks: bool,
    listener: TcpListener,
}

pub struct Visitor {
    settings: Arc<VisitorSettings>,
    dialer: Arc<Dialer>,
    bound: Vec<Bound>,
}

impl Visitor {
    pub fn bind(settings: VisitorSettings) -> Result<Visitor, Failure> {
        let mut bound = Vec::new();
        for (name, forward) in &settings.forwards {
            for (index, local) in forward.listens() {
                let listener = bind_tcp(local).map_err(|e| {
                    crate::server::bind_error(
                        &format!("forward \"{name}\""),
                        Protocol::Tcp,
                        local,
                        &e,
                    )
                })?;
                bound.push(Bound {
                    name: name.clone(),
                    to: forward.to.at(index),
                    socks: forward.socks,
                    listener,
                });
            }
        }
        Ok(Visitor {
            dialer: Arc::new(Dialer::visitor(&settings)),
            settings: Arc::new(settings),
            bound,
        })
    }

    pub fn with_resolver(mut self, resolver: Arc<dyn Resolver>) -> Self {
        Arc::get_mut(&mut self.dialer)
            .expect("set before the visitor runs")
            .resolver = resolver;
        self
    }

    pub fn local_addr(&self, forward: &str) -> Option<SocketAddr> {
        self.bound
            .iter()
            .find(|b| b.name == forward)
            .and_then(|b| b.listener.local_addr().ok())
    }

    pub async fn run(self, mut stop: watch::Receiver<bool>) {
        let (tx, mut rx) = mpsc::channel(64);
        let mut accepting = JoinSet::new();
        for (name, f) in &self.settings.forwards {
            let at = self
                .local_addr(name)
                .map_or_else(|| show_addr(f.local, f.ports), |a| show_addr(a, f.ports));
            let to = match (&f.to, f.ports) {
                (VisitTarget::Client { client, .. }, _) if f.socks => {
                    format!("any target on client \"{client}\" (SOCKS5 proxy)")
                }
                (VisitTarget::Client { client, target }, n) if n > 1 => {
                    let last = f.to.at(n - 1);
                    let end = match &last {
                        VisitTarget::Client { target, .. } => {
                            target.rsplit_once(':').map_or("", |(_, p)| p).to_string()
                        }
                        VisitTarget::Service { .. } => String::new(),
                    };
                    format!("{target}-{end} on client \"{client}\"")
                }
                (to, _) => to.to_string(),
            };
            info!("forwarding {at} -> {to}");
        }
        let forwards = self
            .settings
            .forwards
            .iter()
            .flat_map(|(name, f)| {
                let last = f.ports.max(1) - 1;
                let first = match &f.to {
                    VisitTarget::Client { client, .. } if f.socks => VisitTarget::Client {
                        client: client.clone(),
                        target: "127.0.0.1:1".into(),
                    },
                    to => to.clone(),
                };
                let mut ends = vec![(name.clone(), first)];
                if last > 0 {
                    ends.push((name.clone(), f.to.at(last)));
                }
                ends
            })
            .collect();
        for b in self.bound {
            accepting.spawn(forward::accept(
                b.name,
                b.to,
                b.socks,
                b.listener,
                tx.clone(),
            ));
        }
        drop(tx);
        tokio::spawn(forward::check(self.dialer.clone(), forwards));
        let mut open = JoinSet::new();
        let opening = Arc::new(Semaphore::new(MAX_OPENING));
        loop {
            tokio::select! {
                Some(conn) = rx.recv() => {
                    open.spawn(forward::visit(
                        self.dialer.clone(),
                        self.settings.clone(),
                        opening.clone(),
                        conn,
                    ));
                }
                Some(_) = open.join_next(), if !open.is_empty() => {}
                _ = stop.changed() => break,
            }
        }
        if !open.is_empty() {
            info!("closing {} open connections", open.len());
        }
        info!("visitor stopped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> VisitorSettings {
        let forward = |port: u16, to: VisitTarget| {
            Forward::single(SocketAddr::from(([127, 0, 0, 1], port)), to)
        };
        VisitorSettings {
            server: "tunnel.example.com:7835".into(),
            endpoint: crate::transport::endpoint::Endpoint::parse("tunnel.example.com").unwrap(),
            headers: Vec::new(),
            ca_pem: None,
            name: "laptop".into(),
            secret: crate::secret::Secret::generate(),
            cipher: crate::config::model::Cipher::Chachapoly,
            proxy: None,
            connect_timeout: std::time::Duration::from_secs(1),
            tcp: crate::config::settings::SocketTuning {
                nodelay: true,
                keepalive: None,
            },
            forwards: [
                ("db".to_string(), forward(5432, VisitTarget::service("db"))),
                (
                    "game".to_string(),
                    Forward {
                        ports: 11,
                        ..forward(40000, VisitTarget::service("game"))
                    },
                ),
                (
                    "web".to_string(),
                    forward(8080, VisitTarget::service("web")),
                ),
            ]
            .into(),
        }
    }

    #[test]
    fn one_forward_can_move_to_another_port() {
        let mut s = settings();
        let local = "127.0.0.1:5433".parse().unwrap();
        pick(
            &mut s,
            Pick::Forward {
                name: "db".into(),
                local: Some((local, 1)),
            },
            "visitor.yaml",
        )
        .unwrap();
        assert_eq!(s.forwards.len(), 1);
        assert_eq!(s.forwards["db"].local, local);
    }

    #[test]
    fn a_range_forward_moves_only_to_a_range_of_the_same_size() {
        let local = "127.0.0.1:50000".parse().unwrap();
        let move_to = |ports| {
            pick(
                &mut settings(),
                Pick::Forward {
                    name: "game".into(),
                    local: Some((local, ports)),
                },
                "visitor.yaml",
            )
        };
        assert!(move_to(11).is_ok());
        let err = move_to(1).unwrap_err();
        assert_eq!(
            err.message,
            "forward \"game\" has 11 ports, but --local has 1"
        );
    }

    #[test]
    fn unknown_forwards_get_a_suggestion() {
        let err = pick(
            &mut settings(),
            Pick::Forward {
                name: "dv".into(),
                local: None,
            },
            "visitor.yaml",
        )
        .unwrap_err();
        assert_eq!(err.message, "no forward named \"dv\" in visitor.yaml");
        assert_eq!(
            err.hint.unwrap(),
            "did you mean \"db\"? defined forwards: db, game, web"
        );
        let err = pick(
            &mut settings(),
            Pick::Forward {
                name: "cache".into(),
                local: None,
            },
            "visitor.yaml",
        )
        .unwrap_err();
        assert!(
            err.hint
                .unwrap()
                .ends_with("pass --service cache --local PORT"),
            "an unknown name points to the ad hoc form"
        );
    }

    #[test]
    fn ad_hoc_forwards_replace_the_config() {
        let mut s = settings();
        let to = VisitTarget::Client {
            client: "home".into(),
            target: "127.0.0.1:22".into(),
        };
        let local = "127.0.0.1:2222".parse().unwrap();
        pick(
            &mut s,
            Pick::Adhoc {
                to: to.clone(),
                local: (local, 1),
            },
            "visitor.yaml",
        )
        .unwrap();
        assert_eq!(s.forwards.len(), 1);
        assert_eq!(s.forwards["home"], Forward::single(local, to));
        s.forwards.clear();
        assert!(pick(&mut s, Pick::All, "visitor.yaml").is_err());
    }
}
