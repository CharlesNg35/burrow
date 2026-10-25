use std::collections::BTreeSet;

use burrow_admin_proto::plan::{Impact, Level};
use burrow_config::{SELF, ServerConfig};

use crate::admin::core::ports::LiveSession;
use crate::config::patch::Rename;
use crate::config::settings::{ServerSettings, Service};
use crate::metrics::Status;

fn service_renamed(
    name: &str,
    to: &str,
    svc: &Service,
    next: Option<&Service>,
    open: u64,
) -> (Level, String, Option<String>) {
    let mut msg = format!("renames service {name} to {to}");
    match (svc.bind.is_some(), next.filter(|n| n.bind.is_some())) {
        (true, Some(n)) if ports_of(n) == ports_of(svc) => {
            msg.push_str(&format!("; port {} closes and reopens", ports_of(svc)))
        }
        (true, Some(n)) => msg.push_str(&format!(
            "; port {} closes and {} opens",
            ports_of(svc),
            ports_of(n)
        )),
        (true, None) => msg.push_str(&format!("; port {} closes", ports_of(svc))),
        (false, Some(n)) => msg.push_str(&format!("; port {} opens", ports_of(n))),
        (false, None) => {}
    }
    if open > 0 {
        msg.push_str(&format!("; closes {} on it", connections(open)));
    }
    if !svc.visitors.is_empty() {
        msg.push_str(&format!(
            "; visitors that forward to it need service: {to} in their visitor.yaml"
        ));
    }
    if svc.bind.is_some() || open > 0 {
        (Level::Danger, msg, Some(name.to_string()))
    } else {
        (Level::Warning, msg, None)
    }
}

fn ports_of(svc: &Service) -> String {
    svc.shown_bind().unwrap_or_else(|| "no public port".into())
}

fn connections(n: u64) -> String {
    format!(
        "{n} open {}",
        crate::config::diagnostics::plural(n as usize, "connection")
    )
}

pub fn impacts(
    old: &ServerSettings,
    new: &ServerSettings,
    status: &Status,
    sessions: &[LiveSession],
    restart: &[String],
    renames: &[Rename],
) -> Vec<Impact> {
    let cx = Before {
        old,
        new,
        status,
        sessions,
        renames,
    };
    let mut out = Vec::new();
    service_impacts(&cx, &mut out);
    client_impacts(&cx, &mut out);
    visitor_impacts(&cx, &mut out);
    setting_impacts(&cx, restart, &mut out);
    out.extend(super::web_ports::impacts(old, new));
    out.extend(super::websocket::impacts(old, new, sessions));
    out
}

struct Before<'a> {
    old: &'a ServerSettings,
    new: &'a ServerSettings,
    status: &'a Status,
    sessions: &'a [LiveSession],
    renames: &'a [Rename],
}

impl Before<'_> {
    fn renamed_to(&self, section: &str, name: &str) -> Option<String> {
        self.renames
            .iter()
            .find(|r| r.path.len() == 2 && r.path[0] == section && r.path[1] == name)
            .map(|r| r.to.clone())
    }

    fn renamed_in(&self, section: &str, name: &str) -> bool {
        self.renames
            .iter()
            .any(|r| r.path.len() == 2 && r.path[0] == section && r.to == name)
    }

    fn client_open(&self, name: &str) -> u64 {
        self.status
            .clients
            .iter()
            .find(|c| c.name == name)
            .map_or(0, |c| c.open)
    }
}

fn service_impacts(cx: &Before, out: &mut Vec<Impact>) {
    use burrow_config::{Entry, ServiceEntry};
    let (old, new, status) = (cx.old, cx.new, cx.status);
    let renamed_to = |section: &str, name: &str| cx.renamed_to(section, name);
    let renamed_in = |section: &str, name: &str| cx.renamed_in(section, name);
    let services = ServiceEntry::section();
    let open_of = |name: &str| {
        status
            .services
            .iter()
            .find(|s| s.name == name)
            .map_or(0, |s| s.open)
    };
    let mut retargeted: Vec<String> = Vec::new();
    let mut push = |level: Level, code: &str, message: String, confirm: Option<String>| {
        out.push(Impact {
            level,
            code: code.into(),
            message,
            confirm,
        })
    };
    for (name, svc) in &old.services {
        if let Some(to) = renamed_to(services, name) {
            let (level, msg, confirm) =
                service_renamed(name, &to, svc, new.services.get(&to), open_of(name));
            push(level, "renames", msg, confirm);
            continue;
        }
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
                retargeted.push(name.clone());
            }
            _ => {}
        }
    }
    match retargeted.as_slice() {
        [] => {}
        [one] => push(
            Level::Info,
            "changes-service",
            format!(
                "service {one} uses its new target or clients for new connections; open ones stay"
            ),
            None,
        ),
        [first @ .., last] => push(
            Level::Info,
            "changes-service",
            format!(
                "services {} and {last} use their new target or clients for new connections; open ones stay",
                first.join(", ")
            ),
            None,
        ),
    }
    for (name, svc) in &new.services {
        if !old.services.contains_key(name) && !renamed_in(services, name) {
            let msg = match svc.bind {
                Some(_) => format!("adds service {name}; port {} opens", ports_of(svc)),
                None => format!("adds private service {name}"),
            };
            push(Level::Info, "adds-service", msg, None);
        }
    }
}

fn client_impacts(cx: &Before, out: &mut Vec<Impact>) {
    use burrow_config::{ClientEntry, Entry};
    let (old, new, sessions) = (cx.old, cx.new, cx.sessions);
    let renamed_to = |section: &str, name: &str| cx.renamed_to(section, name);
    let renamed_in = |section: &str, name: &str| cx.renamed_in(section, name);
    let clients = ClientEntry::section();
    let client_open = |name: &str| cx.client_open(name);
    let connected = |name: &str| sessions.iter().find(|s| s.client == name);
    let mut push = |level: Level, code: &str, message: String, confirm: Option<String>| {
        out.push(Impact {
            level,
            code: code.into(),
            message,
            confirm,
        })
    };
    for (name, auth) in &old.clients {
        let live = connected(name);
        if let Some(to) = renamed_to(clients, name) {
            let (level, msg, confirm) = match live {
                Some(_) => {
                    let open = client_open(name);
                    (
                        Level::Danger,
                        format!(
                            "renames client {name} to {to}; it is connected and will be disconnected{}, until its client.yaml says name: {to}",
                            if open > 0 {
                                format!(", closing {}", connections(open))
                            } else {
                                String::new()
                            }
                        ),
                        Some(name.clone()),
                    )
                }
                None => (
                    Level::Warning,
                    format!(
                        "renames client {name} to {to}; it cannot connect until its client.yaml says name: {to}"
                    ),
                    None,
                ),
            };
            push(level, "renames", msg, confirm);
            continue;
        }
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
    for name in new
        .clients
        .keys()
        .filter(|n| !old.clients.contains_key(*n) && !renamed_in(clients, n))
    {
        push(
            Level::Info,
            burrow_admin_proto::codes::ADDS_CLIENT,
            format!("adds client {name}"),
            None,
        );
    }
}

fn visitor_impacts(cx: &Before, out: &mut Vec<Impact>) {
    use burrow_config::{ClientEntry, Entry, ServiceEntry, VisitorEntry};
    let (old, new, status) = (cx.old, cx.new, cx.status);
    let renamed_to = |section: &str, name: &str| cx.renamed_to(section, name);
    let services = ServiceEntry::section();
    let clients = ClientEntry::section();
    let visitors = VisitorEntry::section();
    let mut push = |level: Level, code: &str, message: String, confirm: Option<String>| {
        out.push(Impact {
            level,
            code: code.into(),
            message,
            confirm,
        })
    };
    let visitor_open = |name: &str| {
        status
            .visitors
            .iter()
            .find(|v| v.name == name)
            .map_or(0, |v| v.open)
    };
    for (name, auth) in &old.visitors {
        let open = visitor_open(name);
        let to = renamed_to(visitors, name);
        if let Some(to) = &to {
            let msg = format!(
                "renames visitor {name} to {to}{}; it cannot connect until its visitor.yaml says name: {to}",
                if open > 0 {
                    format!("; closes {}", connections(open))
                } else {
                    String::new()
                }
            );
            let (level, confirm) = if open > 0 {
                (Level::Danger, Some(name.clone()))
            } else {
                (Level::Warning, None)
            };
            push(level, "renames", msg, confirm);
        }
        let now = to.as_deref().unwrap_or(name);
        match new.visitors.get(now) {
            None => {
                let msg = if open > 0 {
                    format!("removes visitor {name}; closes {}", connections(open))
                } else {
                    format!("removes visitor {name}")
                };
                push(Level::Danger, "removes-visitor", msg, Some(name.clone()));
            }
            Some(n) => {
                let had: BTreeSet<String> = old
                    .services
                    .iter()
                    .filter(|(_, s)| s.visitors.contains(name))
                    .map(|(k, _)| renamed_to(services, k).unwrap_or_else(|| k.clone()))
                    .collect();
                let has: BTreeSet<String> = new
                    .services
                    .iter()
                    .filter(|(_, s)| s.visitors.iter().any(|v| v == now))
                    .map(|(k, _)| k.clone())
                    .collect();
                let lost: Vec<&String> = had.difference(&has).collect();
                let lost_reach: Vec<String> = auth
                    .reach
                    .keys()
                    .map(|c| renamed_to(clients, c).unwrap_or_else(|| c.clone()))
                    .filter(|c| !n.reach.contains_key(c))
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
}

fn setting_impacts(cx: &Before, restart: &[String], out: &mut Vec<Impact>) {
    let (old, new, sessions) = (cx.old, cx.new, cx.sessions);
    let client_open = |name: &str| cx.client_open(name);
    let mut push = |level: Level, code: &str, message: String, confirm: Option<String>| {
        out.push(Impact {
            level,
            code: code.into(),
            message,
            confirm,
        })
    };
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
}
