use std::net::SocketAddr;

use burrow_admin_proto::codes::{OPENS_PORT, PORT_IN_USE, STOPS_WEBSITES, WEB_PORT_SHARED};
use burrow_admin_proto::diag::{Diag, Severity};
use burrow_admin_proto::plan::{Impact, Level};
use burrow_config::{Protocol, ServerConfig, ServiceEntry, WebConfig, WebSocketConfig};

use super::websocket::over_websocket;
use crate::admin::core::ports::LiveSession;
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

    fn word(self) -> &'static str {
        match self {
            Port::Https => "HTTPS",
            Port::Http => "HTTP",
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

pub fn shared(
    old: &ServerSettings,
    (before, after): (&serde_json::Value, &serde_json::Value),
    sessions: &[LiveSession],
) -> Vec<Impact> {
    let Some(ws) = &old.websocket else {
        return Vec::new();
    };
    Port::BOTH
        .into_iter()
        .filter(|p| ws.shares == p.shares() && raw_at(before, *p) != raw_at(after, *p))
        .filter_map(|p| {
            let addr = p.of(old)?;
            Some(Impact {
                level: Level::Warning,
                code: WEB_PORT_SHARED.into(),
                message: format!(
                    "moves or removes {} {addr}, which the tunnel shares (websocket.listen); {}",
                    p.key(),
                    super::websocket::cut_off(&over_websocket(sessions))
                ),
                confirm: Some(ServerConfig::WEBSOCKET.key().into()),
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
                "opens the web proxy's {} port {} on {} ({})",
                p.word(),
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

fn held(s: &ServerSettings) -> Vec<(Protocol, SocketAddr)> {
    let mut out: Vec<(Protocol, SocketAddr)> = s
        .listen
        .into_iter()
        .chain(s.metrics)
        .chain(s.websocket.as_ref().map(|w| w.listen))
        .chain(Port::BOTH.into_iter().filter_map(|p| p.of(s)))
        .map(|a| (Protocol::Tcp, a))
        .collect();
    for svc in s.services.values() {
        out.extend(svc.binds().into_iter().map(|(_, a)| (svc.protocol, a)));
    }
    out
}

type Wanted = (Vec<String>, Protocol, SocketAddr);

fn wanted(running: &ServerSettings, next: &ServerSettings) -> Vec<Wanted> {
    let mut out: Vec<Wanted> = Port::BOTH
        .into_iter()
        .filter_map(|p| {
            let addr = p.of(next).filter(|a| p.of(running) != Some(*a))?;
            Some((p.path(), Protocol::Tcp, addr))
        })
        .collect();
    if let Some(addr) = next.listen.filter(|a| running.listen != Some(*a)) {
        out.push((ServerConfig::LISTEN.path(), Protocol::Tcp, addr));
    }
    let own = |s: &ServerSettings| {
        s.websocket
            .as_ref()
            .filter(|w| w.shares == Shares::Nothing)
            .map(|w| w.listen)
    };
    if let Some(addr) = own(next).filter(|a| own(running) != Some(*a)) {
        out.push((
            ServerConfig::WEBSOCKET.then(WebSocketConfig::LISTEN).path(),
            Protocol::Tcp,
            addr,
        ));
    }
    for (name, svc) in &next.services {
        let before = running.services.get(name).map(|s| (s.protocol, s.binds()));
        if before == Some((svc.protocol, svc.binds())) {
            continue;
        }
        out.extend(
            svc.binds()
                .into_iter()
                .map(|(_, a)| (ServiceEntry::BIND.of(name), svc.protocol, a)),
        );
    }
    out
}

pub fn busy(
    probe: &dyn Fn(Protocol, SocketAddr) -> std::io::Result<()>,
    (running, before): (&ServerSettings, &ServerSettings),
    next: &ServerSettings,
) -> Vec<Diag> {
    let ours = held(running);
    wanted(before, next)
        .into_iter()
        .filter_map(|(path, proto, addr)| {
            if ours
                .iter()
                .any(|(p, h)| *p == proto && binds_conflict(addr, *h))
            {
                return None;
            }
            let e = probe(proto, addr).err()?;
            if e.kind() == std::io::ErrorKind::AddrNotAvailable
                || e.kind() == std::io::ErrorKind::PermissionDenied
                    && crate::config::check::low_port_refused(addr)
            {
                return None;
            }
            let f = crate::server::bind_error(&path.join("."), proto, addr, &e);
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
                end_line: None,
                end_col: None,
                before: false,
            })
        })
        .fold(Vec::new(), |mut out: Vec<Diag>, d| {
            if out.last().is_none_or(|l| l.path != d.path) {
                out.push(d);
            }
            out
        })
}
