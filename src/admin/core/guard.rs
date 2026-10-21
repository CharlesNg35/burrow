mod grants;

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::Path;

use burrow_admin_proto::plan::{Impact, Level, Refusal};

use super::ports::LiveSession;
use crate::config::patch::Rename;
use crate::config::settings::{ServerSettings, Service};
use crate::metrics::Status;
use crate::secret::Secret;
use burrow_admin_proto::msg::AdminLevel;
use burrow_config::{SELF, ServerConfig};

pub struct Who<'a> {
    pub visitor: &'a str,
    pub peer: SocketAddr,
    pub secret: &'a Secret,
    pub via_websocket: bool,
}

pub enum Base<'a> {
    Valid(&'a ServerSettings),
    Broken {
        raw: &'a serde_json::Value,
        next_raw: &'a serde_json::Value,
    },
}

pub struct Change<'a> {
    pub running: &'a ServerSettings,
    pub base: Base<'a>,
    pub next: &'a ServerSettings,
    pub renames: &'a [Rename],
    pub path: &'a Path,
}

fn refusal(code: &str, message: String) -> Refusal {
    Refusal {
        code: code.into(),
        message,
    }
}

pub fn refusals(who: &Who, change: &Change) -> Vec<Refusal> {
    let (old, new) = (change.running, change.next);
    let mut out = Vec::new();
    let me = who.visitor;
    match new.visitors.get(me) {
        None => out.push(refusal(
            "self-removed",
            format!("this change removes visitor \"{me}\", which you are signed in as; you would lose access"),
        )),
        Some(v) => {
            if v.admin != Some(AdminLevel::Write) {
                out.push(refusal(
                    "self-demoted",
                    format!("this change takes write access away from visitor \"{me}\", which you are signed in as"),
                ));
            }
            if !v.secrets.contains(who.secret) {
                out.push(refusal(
                    "self-secret",
                    format!(
                        "this change removes secret {}, which this session signed in with",
                        who.secret.fingerprint()
                    ),
                ));
            }
            if v.admin.is_some() && !v.admits_admin(who.peer.ip()) {
                out.push(refusal(
                    "self-source",
                    format!(
                        "admin_from of visitor \"{me}\" would no longer admit your address {}",
                        who.peer.ip()
                    ),
                ));
            }
        }
    }
    if !new
        .visitors
        .values()
        .any(|v| v.admin == Some(AdminLevel::Write))
        && out
            .iter()
            .all(|r| r.code != "self-removed" && r.code != "self-demoted")
    {
        out.push(refusal(
            "last-writer",
            "no visitor would keep write access, so nobody could change the config from burrow admin".into(),
        ));
    }
    if who.via_websocket {
        let kept =
            matches!((&old.websocket, &new.websocket), (Some(a), Some(b)) if a.path == b.path);
        if !kept {
            out.push(refusal(
                "self-transport",
                "this session comes through the WebSocket listener, and this change removes it or changes its path".into(),
            ));
        }
    } else if new.listen.is_none() && old.listen.is_some() {
        out.push(refusal(
            "self-transport",
            "this session comes through the TCP listener, which this change removes".into(),
        ));
    }
    out.extend(grants::refusals(change));
    out
}

fn ports_of(svc: &Service) -> String {
    svc.shown_bind().unwrap_or_else(|| "no public port".into())
}

fn connections(n: u64) -> String {
    format!(
        "{n} open {}",
        crate::diagnostics::plural(n as usize, "connection")
    )
}

pub fn impacts(
    old: &ServerSettings,
    new: &ServerSettings,
    status: &Status,
    sessions: &[LiveSession],
    restart: &[String],
) -> Vec<Impact> {
    let mut out = Vec::new();
    let open_of = |name: &str| {
        status
            .services
            .iter()
            .find(|s| s.name == name)
            .map_or(0, |s| s.open)
    };
    let client_open = |name: &str| {
        status
            .clients
            .iter()
            .find(|c| c.name == name)
            .map_or(0, |c| c.open)
    };
    let connected = |name: &str| sessions.iter().find(|s| s.client == name);
    let mut push = |level: Level, code: &str, message: String, confirm: Option<String>| {
        out.push(Impact {
            level,
            code: code.into(),
            message,
            confirm,
        })
    };
    for (name, svc) in &old.services {
        match new.services.get(name) {
            None => {
                let open = open_of(name);
                let mut msg = format!("removes service {name}");
                if svc.bind.is_some() {
                    msg.push_str(&format!("; port {} closes", ports_of(svc)));
                }
                if open > 0 {
                    msg.push_str(&format!("; closes {} on it", connections(open)));
                }
                push(Level::Danger, "removes-service", msg, Some(name.clone()));
            }
            Some(n) if n.bind != svc.bind || n.ports != svc.ports || n.protocol != svc.protocol => {
                let open = open_of(name);
                let msg = format!(
                    "moves service {name} from {} to {}{}",
                    ports_of(svc),
                    ports_of(n),
                    if open > 0 {
                        format!("; closes {}", connections(open))
                    } else {
                        String::new()
                    }
                );
                let confirm = (open > 0).then(|| name.clone());
                let level = if open > 0 {
                    Level::Danger
                } else {
                    Level::Warning
                };
                push(level, "rebinds-service", msg, confirm);
            }
            Some(n) if n.target != svc.target || n.clients != svc.clients => {
                push(
                    Level::Info,
                    "changes-service",
                    format!(
                        "service {name} uses its new target or clients for new connections; open ones stay"
                    ),
                    None,
                );
            }
            _ => {}
        }
    }
    for (name, svc) in &new.services {
        if !old.services.contains_key(name) {
            let msg = match svc.bind {
                Some(_) => format!("adds service {name}; port {} opens", ports_of(svc)),
                None => format!("adds private service {name}"),
            };
            push(Level::Info, "adds-service", msg, None);
        }
    }
    for (name, auth) in &old.clients {
        let live = connected(name);
        match new.clients.get(name) {
            None => {
                let mut msg = format!("removes client {name}");
                if live.is_some() {
                    msg.push_str("; it is connected and will be disconnected");
                    let open = client_open(name);
                    if open > 0 {
                        msg.push_str(&format!(", closing {}", connections(open)));
                    }
                }
                push(Level::Danger, "removes-client", msg, Some(name.clone()));
            }
            Some(n) => {
                if let Some(s) = live {
                    let kept = n.secrets.iter().any(|x| x.fingerprint() == s.secret_fp);
                    let had = auth.secrets.iter().any(|x| x.fingerprint() == s.secret_fp);
                    if had && !kept {
                        let open = client_open(name);
                        push(
                            Level::Danger,
                            "disconnects-client",
                            format!(
                                "client {name} is connected with secret {}, which this change removes; it will be disconnected{}",
                                s.secret_fp,
                                if open > 0 {
                                    format!(", closing {}", connections(open))
                                } else {
                                    String::new()
                                }
                            ),
                            Some(name.clone()),
                        );
                    }
                }
            }
        }
    }
    for name in new.clients.keys().filter(|n| !old.clients.contains_key(*n)) {
        push(
            Level::Info,
            "adds-client",
            format!("adds client {name}"),
            None,
        );
    }
    let visitor_open = |name: &str| {
        status
            .visitors
            .iter()
            .find(|v| v.name == name)
            .map_or(0, |v| v.open)
    };
    for (name, auth) in &old.visitors {
        let open = visitor_open(name);
        match new.visitors.get(name) {
            None => {
                let msg = if open > 0 {
                    format!("removes visitor {name}; closes {}", connections(open))
                } else {
                    format!("removes visitor {name}")
                };
                push(Level::Danger, "removes-visitor", msg, Some(name.clone()));
            }
            Some(n) => {
                let had: BTreeSet<&String> = old
                    .services
                    .iter()
                    .filter(|(_, s)| s.visitors.contains(name))
                    .map(|(k, _)| k)
                    .collect();
                let has: BTreeSet<&String> = new
                    .services
                    .iter()
                    .filter(|(_, s)| s.visitors.contains(name))
                    .map(|(k, _)| k)
                    .collect();
                let lost: Vec<&&String> = had.difference(&has).collect();
                let lost_reach: Vec<&String> = auth
                    .reach
                    .keys()
                    .filter(|c| !n.reach.contains_key(*c))
                    .collect();
                if !lost.is_empty() || !lost_reach.is_empty() {
                    let mut what: Vec<String> =
                        lost.iter().map(|s| format!("service {s}")).collect();
                    what.extend(lost_reach.iter().map(|c| format!("client {c}")));
                    let msg = format!(
                        "visitor {name} loses access to {}{}",
                        what.join(", "),
                        if open > 0 {
                            format!("; up to {} close", connections(open))
                        } else {
                            String::new()
                        }
                    );
                    let level = if open > 0 {
                        Level::Danger
                    } else {
                        Level::Warning
                    };
                    push(
                        level,
                        "visitor-loses-access",
                        msg,
                        (open > 0).then(|| name.clone()),
                    );
                }
            }
        }
    }
    let reaches_self = |s: &ServerSettings| s.visitors.values().any(|v| v.reach.contains_key(SELF));
    if old.self_allow.is_some() && new.self_allow != old.self_allow && reaches_self(old) {
        let open = client_open(SELF);
        push(
            if open > 0 {
                Level::Warning
            } else {
                Level::Info
            },
            "changes-self-allow",
            format!(
                "changes self.allow; of the {} to this server, those the new list no longer permits close",
                connections(open)
            ),
            None,
        );
    }
    if new.listen != old.listen {
        push(
            Level::Danger,
            "changes-listen",
            format!(
                "changes the TCP listener from {} to {}; it takes effect when the server restarts",
                old.listen.map_or_else(|| "off".into(), |a| a.to_string()),
                new.listen.map_or_else(|| "off".into(), |a| a.to_string())
            ),
            Some(ServerConfig::LISTEN.key().into()),
        );
    }
    let ws = |s: &ServerSettings| s.websocket.as_ref().map(|w| (w.listen, w.path.clone()));
    if ws(old) != ws(new) {
        push(
            Level::Danger,
            "changes-websocket",
            "changes the WebSocket listener; clients connected through it reconnect".to_string(),
            Some(ServerConfig::WEBSOCKET.key().into()),
        );
    }
    if new.insecure_plaintext != old.insecure_plaintext {
        let n = sessions.len();
        push(
            Level::Danger,
            "plaintext-toggled",
            format!(
                "{} encryption of forwarded data; every connected client ({n}) reconnects",
                if new.insecure_plaintext {
                    "turns off"
                } else {
                    "turns on"
                }
            ),
            Some(ServerConfig::INSECURE_PLAINTEXT.key().into()),
        );
    }
    for field in restart {
        if field != ServerConfig::LISTEN.key() {
            push(
                Level::Info,
                "restart-needed",
                format!("{field} takes effect when the server restarts"),
                None,
            );
        }
    }
    out
}

#[cfg(test)]
mod tests;
