mod forward;
mod socks;

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::sync::{Semaphore, mpsc, watch};
use tokio::task::JoinSet;
use tracing::{error, info};

use crate::config::check::ports::show_addr;
use crate::config::model::Protocol;
use crate::config::settings::{Forward, VisitorSettings};
use crate::dialer::Dialer;
use crate::failure::Failure;
use crate::protocol::VisitTarget;
use crate::transport::resolve::Resolver;
use crate::transport::tcp::bind_tcp;

const MAX_OPENING: usize = 32;

#[derive(Clone)]
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

fn bind_forward(name: &str, forward: &Forward) -> Result<Vec<Bound>, Failure> {
    forward
        .listens()
        .into_iter()
        .map(|(index, local)| {
            let listener = bind_tcp(local).map_err(|e| {
                crate::server::bind_error(&format!("forward \"{name}\""), Protocol::Tcp, local, &e)
            })?;
            Ok(Bound {
                name: name.to_string(),
                to: forward.to.at(index),
                socks: forward.socks,
                listener,
            })
        })
        .collect()
}

fn describe(forward: &Forward, at: SocketAddr) -> String {
    let at = show_addr(at, forward.ports);
    let to = match (&forward.to, forward.ports) {
        (VisitTarget::Client { client, .. }, _) if forward.socks => {
            format!("any target on client \"{client}\" (SOCKS5 proxy)")
        }
        (VisitTarget::Client { client, target }, n) if n > 1 => {
            let end = match forward.to.at(n - 1) {
                VisitTarget::Client { target, .. } => {
                    target.rsplit_once(':').map_or("", |(_, p)| p).to_string()
                }
                VisitTarget::Service { .. } => String::new(),
            };
            format!("{target}-{end} on client \"{client}\"")
        }
        (to, _) => to.to_string(),
    };
    format!("forwarding {at} -> {to}")
}

fn ends<'a>(
    forwards: impl Iterator<Item = (&'a String, &'a Forward)>,
) -> Vec<(String, VisitTarget)> {
    forwards
        .flat_map(|(name, f)| {
            let first = match &f.to {
                VisitTarget::Client { client, .. } if f.socks => VisitTarget::Client {
                    client: client.clone(),
                    target: String::new(),
                },
                to => to.clone(),
            };
            let last = f.ports.max(1) - 1;
            let mut ends = vec![(name.clone(), first)];
            if last > 0 {
                ends.push((name.clone(), f.to.at(last)));
            }
            ends
        })
        .collect()
}

fn connection_changes(old: &VisitorSettings, new: &VisitorSettings) -> Vec<&'static str> {
    let VisitorSettings {
        server,
        endpoint,
        headers,
        ca_pem,
        ca_file: _,
        name,
        secret,
        cipher,
        proxy,
        connect_timeout,
        tcp,
        forwards: _,
    } = new;
    [
        (*server != old.server || *endpoint != old.endpoint, "server"),
        (*name != old.name, "name"),
        (*secret != old.secret, "secret"),
        (*headers != old.headers, "headers"),
        (*ca_pem != old.ca_pem, "ca"),
        (*cipher != old.cipher, "cipher"),
        (*proxy != old.proxy, "proxy"),
        (
            *connect_timeout != old.connect_timeout,
            "connect_timeout_secs",
        ),
        (*tcp != old.tcp, "tcp"),
    ]
    .into_iter()
    .filter_map(|(changed, name)| changed.then_some(name))
    .collect()
}

struct Listening {
    forward: Forward,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Listening {
    async fn close(self) {
        for task in &self.tasks {
            task.abort();
        }
        for task in self.tasks {
            let _ = task.await;
        }
    }
}

fn dialer_for(settings: &VisitorSettings, resolver: &Option<Arc<dyn Resolver>>) -> Arc<Dialer> {
    let mut dialer = Dialer::visitor(settings);
    if let Some(r) = resolver {
        dialer.resolver = r.clone();
    }
    Arc::new(dialer)
}

pub struct Visitor {
    settings: Arc<VisitorSettings>,
    dialer: Arc<Dialer>,
    resolver: Option<Arc<dyn Resolver>>,
    bound: Vec<Bound>,
    reloads: Option<mpsc::Receiver<VisitorSettings>>,
}

impl Visitor {
    pub fn bind(settings: VisitorSettings) -> Result<Visitor, Failure> {
        let mut bound = Vec::new();
        for (name, forward) in &settings.forwards {
            bound.extend(bind_forward(name, forward)?);
        }
        Ok(Visitor {
            dialer: dialer_for(&settings, &None),
            settings: Arc::new(settings),
            resolver: None,
            bound,
            reloads: None,
        })
    }

    pub fn with_resolver(mut self, resolver: Arc<dyn Resolver>) -> Self {
        self.resolver = Some(resolver);
        self.dialer = dialer_for(&self.settings, &self.resolver);
        self
    }

    pub fn with_reloads(mut self, reloads: mpsc::Receiver<VisitorSettings>) -> Self {
        self.reloads = Some(reloads);
        self
    }

    pub fn local_addr(&self, forward: &str) -> Option<SocketAddr> {
        self.bound
            .iter()
            .find(|b| b.name == forward)
            .and_then(|b| b.listener.local_addr().ok())
    }

    fn listen(
        bound: Vec<Bound>,
        tx: &mpsc::Sender<forward::Incoming>,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        bound
            .into_iter()
            .map(|b| {
                tokio::spawn(forward::accept(
                    b.name,
                    b.to,
                    b.socks,
                    b.listener,
                    tx.clone(),
                ))
            })
            .collect()
    }

    pub async fn run(mut self, mut stop: watch::Receiver<bool>) {
        let (tx, mut rx) = mpsc::channel(64);
        let mut listening = std::collections::BTreeMap::new();
        for (name, f) in self.settings.forwards.clone() {
            let at = self.local_addr(&name).unwrap_or(f.local);
            info!("{}", describe(&f, at));
            let bound: Vec<Bound> = self.bound.extract_if(.., |b| b.name == name).collect();
            listening.insert(
                name,
                Listening {
                    forward: f,
                    tasks: Self::listen(bound, &tx),
                },
            );
        }
        tokio::spawn(forward::check(
            self.dialer.clone(),
            ends(self.settings.forwards.iter()),
        ));
        let mut reloads = self.reloads.take();
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
                next = next_reload(&mut reloads) => {
                    self.apply(next, &mut listening, &tx).await;
                }
                _ = stop.changed() => break,
            }
        }
        for (_, l) in std::mem::take(&mut listening) {
            l.close().await;
        }
        if !open.is_empty() {
            info!("closing {} open connections", open.len());
        }
        info!("visitor stopped");
    }

    async fn apply(
        &mut self,
        next: VisitorSettings,
        listening: &mut std::collections::BTreeMap<String, Listening>,
        tx: &mpsc::Sender<forward::Incoming>,
    ) {
        let connection = connection_changes(&self.settings, &next);
        let gone: Vec<String> = listening
            .iter()
            .filter(|(name, l)| next.forwards.get(*name) != Some(&l.forward))
            .map(|(name, _)| name.clone())
            .collect();
        let mut forwards: Vec<&str> = gone
            .iter()
            .map(String::as_str)
            .chain(
                next.forwards
                    .keys()
                    .filter(|n| !listening.contains_key(*n))
                    .map(String::as_str),
            )
            .collect();
        forwards.sort_unstable();
        forwards.dedup();
        info!("{}", summary(&connection, &forwards));
        for name in &gone {
            if let Some(l) = listening.remove(name) {
                l.close().await;
            }
            if !next.forwards.contains_key(name) {
                info!("forward \"{name}\" removed");
            }
        }
        if !connection.is_empty() {
            self.dialer = dialer_for(&next, &self.resolver);
        }
        let mut added = Vec::new();
        for (name, f) in &next.forwards {
            if listening.contains_key(name) {
                continue;
            }
            match bind_forward(name, f) {
                Ok(bound) => {
                    let at = bound
                        .first()
                        .and_then(|b| b.listener.local_addr().ok())
                        .unwrap_or(f.local);
                    info!("{}", describe(f, at));
                    listening.insert(
                        name.clone(),
                        Listening {
                            forward: f.clone(),
                            tasks: Self::listen(bound, tx),
                        },
                    );
                    added.push((name, f));
                }
                Err(e) => error!(
                    "{}; forward \"{name}\" stays off until the config is fixed",
                    e.message
                ),
            }
        }
        let check = if connection.is_empty() {
            ends(added.into_iter())
        } else {
            ends(next.forwards.iter())
        };
        self.settings = Arc::new(next);
        if !check.is_empty() {
            tokio::spawn(forward::check(self.dialer.clone(), check));
        }
    }
}

fn summary(connection: &[&str], forwards: &[&str]) -> String {
    let mut parts = Vec::new();
    if !connection.is_empty() {
        parts.push(format!(
            "{} applied to new connections, open ones keep running",
            connection.join(", ")
        ));
    }
    match forwards {
        [] => {}
        [one] => parts.push(format!("forward {one} changed")),
        many => parts.push(format!("forwards {} changed", many.join(", "))),
    }
    if parts.is_empty() {
        return "config reloaded: nothing changed".into();
    }
    format!("config reloaded: {}", parts.join("; "))
}

async fn next_reload(reloads: &mut Option<mpsc::Receiver<VisitorSettings>>) -> VisitorSettings {
    loop {
        match reloads {
            Some(rx) => match rx.recv().await {
                Some(next) => return next,
                None => *reloads = None,
            },
            None => std::future::pending::<()>().await,
        }
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
            ca_file: None,
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
    fn connection_changes_name_each_field_and_ignore_forwards() {
        let old = settings();
        let mut new = old.clone();
        new.forwards.clear();
        assert!(connection_changes(&old, &new).is_empty());
        new.secret = crate::secret::Secret::generate();
        new.connect_timeout = std::time::Duration::from_secs(9);
        new.tcp.nodelay = false;
        assert_eq!(
            connection_changes(&old, &new),
            vec!["secret", "connect_timeout_secs", "tcp"]
        );
        assert_eq!(
            summary(&["secret"], &["db", "web"]),
            "config reloaded: secret applied to new connections, open ones keep running; forwards db, web changed"
        );
        assert_eq!(summary(&[], &[]), "config reloaded: nothing changed");
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
