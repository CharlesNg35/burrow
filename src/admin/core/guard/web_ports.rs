use std::net::SocketAddr;

use burrow_admin_proto::codes::{OPENS_PORT, PORT_IN_USE, STOPS_WEBSITES, WEB_PORT_SHARED};
use burrow_admin_proto::diag::{Diag, Severity};
use burrow_admin_proto::plan::{Impact, Level, Refusal};
use burrow_config::{Protocol, ServerConfig, WebConfig, WebSocketConfig};

use crate::config::check::binds_conflict;
use crate::config::settings::{ServerSettings, Shares, WebTls};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Port {
    Https,
    Http,
}

impl Port {
    const BOTH: [Port; 2] = [Port::Https, Port::Http];

    fn path(self) -> Vec<String> {
        match self {
            Port::Https => ServerConfig::WEB.then(WebConfig::HTTPS).path(),
            Port::Http => ServerConfig::WEB.then(WebConfig::HTTP).path(),
        }
    }

    fn key(self) -> String {
        self.path().join(".")
    }

    fn of(self, s: &ServerSettings) -> Option<SocketAddr> {
        let web = s.web.as_ref()?;
        match self {
            Port::Https => web.https,
            Port::Http => web.http,
        }
    }

    fn shares(self) -> Shares {
        match self {
            Port::Https => Shares::Https,
            Port::Http => Shares::Http,
        }
    }

    fn serves(self, tls: &WebTls) -> bool {
        (*tls == WebTls::Off) == (self == Port::Http)
    }
}

fn raw_at(v: &serde_json::Value, p: Port) -> Option<&serde_json::Value> {
    p.path().iter().try_fold(v, |v, k| v.get(k))
}

pub fn refusals(
    old: &ServerSettings,
    (before, after): (&serde_json::Value, &serde_json::Value),
) -> Vec<Refusal> {
    let Some(ws) = &old.websocket else {
        return Vec::new();
    };
    Port::BOTH
        .into_iter()
        .filter(|p| ws.shares == p.shares() && raw_at(before, *p) != raw_at(after, *p))
        .filter_map(|p| {
            let addr = p.of(old)?;
            Some(Refusal {
                code: WEB_PORT_SHARED.into(),
                message: format!(
                    "{} {addr} is shared with the tunnel (websocket.listen); moving or removing it disconnects every client, so change both in the config file on the server",
                    p.key()
                ),
            })
        })
        .collect()
}

fn addresses(addr: SocketAddr) -> String {
    if addr.ip().is_unspecified() {
        "all addresses".into()
    } else {
        addr.ip().to_string()
    }
}

fn stopped(old: &ServerSettings, new: &ServerSettings) -> Vec<String> {
    let Some(web) = &old.web else {
        return Vec::new();
    };
    let gone = |p: Port| p.of(old).is_some() && p.of(new).is_none();
    let whole = new.web.is_none();
    web.sites
        .iter()
        .filter(|(_, site)| {
            whole
                || Port::BOTH
                    .into_iter()
                    .any(|p| gone(p) && p.serves(&site.tls))
        })
        .map(|(name, _)| name.clone())
        .collect()
}

pub fn impacts(old: &ServerSettings, new: &ServerSettings) -> Vec<Impact> {
    let mut out = Vec::new();
    for p in Port::BOTH {
        let Some(addr) = p.of(new).filter(|a| p.of(old) != Some(*a)) else {
            continue;
        };
        out.push(Impact {
            level: if addr.ip().is_unspecified() {
                Level::Warning
            } else {
                Level::Info
            },
            code: OPENS_PORT.into(),
            message: format!(
                "opens port {} on {} for websites ({})",
                addr.port(),
                addresses(addr),
                p.key()
            ),
            confirm: None,
        });
    }
    let stops = stopped(old, new);
    if !stops.is_empty() {
        out.push(Impact {
            level: Level::Warning,
            code: STOPS_WEBSITES.into(),
            message: format!("stops websites {}", stops.join(", ")),
            confirm: None,
        });
    }
    out
}

fn held(s: &ServerSettings) -> Vec<SocketAddr> {
    let mut out: Vec<SocketAddr> = s.listen.into_iter().chain(s.metrics).collect();
    out.extend(s.websocket.as_ref().map(|w| w.listen));
    out.extend(Port::BOTH.into_iter().filter_map(|p| p.of(s)));
    for svc in s.services.values() {
        if svc.protocol == Protocol::Tcp {
            out.extend(svc.binds().into_iter().map(|(_, a)| a));
        }
    }
    out
}

fn wanted(running: &ServerSettings, next: &ServerSettings) -> Vec<(Vec<String>, SocketAddr)> {
    let mut out: Vec<(Vec<String>, SocketAddr)> = Port::BOTH
        .into_iter()
        .filter_map(|p| {
            let addr = p.of(next).filter(|a| p.of(running) != Some(*a))?;
            Some((p.path(), addr))
        })
        .collect();
    let own = |s: &ServerSettings| {
        s.websocket
            .as_ref()
            .filter(|w| w.shares == Shares::Nothing)
            .map(|w| w.listen)
    };
    if let Some(addr) = own(next).filter(|a| own(running) != Some(*a)) {
        out.push((
            ServerConfig::WEBSOCKET.then(WebSocketConfig::LISTEN).path(),
            addr,
        ));
    }
    out
}

pub fn busy(
    probe: &dyn Fn(SocketAddr) -> std::io::Result<()>,
    running: &ServerSettings,
    next: &ServerSettings,
) -> Vec<Diag> {
    let ours = held(running);
    wanted(running, next)
        .into_iter()
        .filter_map(|(path, addr)| {
            if ours.iter().any(|h| binds_conflict(addr, *h)) {
                return None;
            }
            let e = probe(addr).err()?;
            let f = crate::server::bind_error(&path.join("."), Protocol::Tcp, addr, &e);
            Some(Diag {
                severity: Severity::Error,
                code: PORT_IN_USE.into(),
                message: f.message,
                path: Some(path),
                also: Vec::new(),
                subjects: Vec::new(),
                file: None,
                label: None,
                help: f.hint,
                line: None,
                col: None,
                before: false,
            })
        })
        .collect()
}
