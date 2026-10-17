use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use super::super::model::{ForwardEntry, VisitorConfig};
use super::super::settings::{Forward, VisitorSettings};
use super::connection::{Connection, Dial, check_connection};
use super::{
    MAX_TARGET_LEN, Spans, binds_conflict, name_error, parse_host_port, parse_local, secret_error,
    tuning, valid_name,
};
use crate::diagnostics::Diagnostic;
use crate::protocol::VisitTarget;
use crate::secret::Secret;

pub fn check_visitor(
    cfg: &VisitorConfig,
    spans: &Spans,
    base_dir: &Path,
    out: &mut Vec<Diagnostic>,
) -> Option<VisitorSettings> {
    let errors_before = out.iter().filter(|d| d.is_error()).count();
    let Connection {
        endpoint,
        headers,
        ca_pem,
        proxy,
    } = check_connection(
        Dial {
            server: &cfg.server,
            headers: &cfg.headers,
            ca: cfg.ca.as_deref(),
            proxy: cfg.proxy.as_deref(),
        },
        spans,
        base_dir,
        out,
    );
    if !valid_name(&cfg.name) {
        out.push(name_error("visitor", &cfg.name, spans.value(&["name"])));
    }
    let secret = match Secret::parse(&cfg.secret) {
        Ok(s) => Some(s),
        Err(e) => {
            out.push(secret_error(
                &format!("visitor \"{}\"", cfg.name),
                &e,
                spans.value(&["secret"]),
            ));
            None
        }
    };
    if cfg.forwards.is_empty() {
        let d = Diagnostic::warning("no-forwards", "no forwards are defined")
            .help(format!(
                "add one, for example:\n         forwards:\n           db: {{ service: db, local: 5432 }}\n       or name the target when you run it: {}",
                crate::failure::cmd("visit --service NAME --local PORT")
            ));
        out.push(if spans.has(&["forwards"]) {
            d.at(spans.key(&["forwards"]))
        } else {
            d
        });
    }

    let mut forwards = BTreeMap::new();
    let mut locals: Vec<(SocketAddr, &str)> = Vec::new();
    for (name, f) in &cfg.forwards {
        if !valid_name(name) {
            out.push(name_error("forward", name, spans.key(&["forwards", name])));
        }
        let local = check_local(name, f, spans, out);
        if let Some(l) = local {
            if let Some((_, other)) = locals.iter().find(|(a, _)| binds_conflict(*a, l)) {
                out.push(
                    Diagnostic::error(
                        "duplicate-bind",
                        format!(
                            "forward \"{name}\" listens on {l}, already used by forward \"{other}\""
                        ),
                    )
                    .at(spans.value(&["forwards", name, "local"]))
                    .label("address already used")
                    .help("pick a different local port for one of the forwards"),
                );
            }
            locals.push((l, name));
        }
        let to = check_destination(name, f, spans, out);
        if let (Some(local), Some(to)) = (local, to) {
            forwards.insert(name.clone(), Forward { local, to });
        }
    }

    if out.iter().filter(|d| d.is_error()).count() > errors_before {
        return None;
    }
    let endpoint = endpoint?;
    Some(VisitorSettings {
        server: endpoint.to_string(),
        endpoint,
        headers,
        ca_pem,
        name: cfg.name.clone(),
        secret: secret?,
        cipher: cfg.cipher,
        proxy,
        connect_timeout: Duration::from_secs(cfg.connect_timeout_secs.into()),
        tcp: tuning(&cfg.tcp),
        forwards,
    })
}

fn check_local(
    name: &str,
    f: &ForwardEntry,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Option<SocketAddr> {
    let span = spans.value(&["forwards", name, "local"]);
    match parse_local(&f.local) {
        Ok(l) => {
            if l.ip().is_unspecified() {
                out.push(
                    Diagnostic::warning(
                        "shared-forward",
                        format!("forward \"{name}\" listens on all interfaces"),
                    )
                    .at(span)
                    .label("reachable from your network")
                    .help(format!(
                        "use \"local: {}\" (127.0.0.1 only) unless other machines should use it: they would get this visitor's access",
                        l.port()
                    )),
                );
            }
            Some(l)
        }
        Err(e) => {
            out.push(
                Diagnostic::error(
                    "invalid-address",
                    format!("invalid local address for forward \"{name}\": {e}"),
                )
                .at(span)
                .label("expected a port or ip:port")
                .help("use a port like 5432 (on 127.0.0.1), or ip:port like 127.0.0.1:5432"),
            );
            None
        }
    }
}

fn check_destination(
    name: &str,
    f: &ForwardEntry,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Option<VisitTarget> {
    let path = |k: &'static str| ["forwards", name, k];
    let problem = |message: String, label: &'static str, help: &str| {
        Diagnostic::error("invalid-forward", message)
            .at(spans.key(&["forwards", name]))
            .label(label)
            .help(help.to_string())
    };
    let both = "use \"service\" for a private service, or \"client\" and \"target\" to reach a client directly";
    match (&f.service, &f.client, &f.target) {
        (Some(service), None, None) => {
            if !valid_name(service) {
                out.push(name_error(
                    "service",
                    service,
                    spans.value(&path("service")),
                ));
                return None;
            }
            Some(VisitTarget::Service(service.clone()))
        }
        (None, Some(client), Some(target)) => {
            if !valid_name(client) {
                out.push(name_error("client", client, spans.value(&path("client"))));
            }
            let target = target.host_port();
            let checked = if target.len() > MAX_TARGET_LEN {
                Err(format!(
                    "\"{target}\" is longer than {MAX_TARGET_LEN} characters"
                ))
            } else {
                parse_host_port(&target).map(drop)
            };
            if let Err(e) = checked {
                out.push(
                    Diagnostic::error(
                        "invalid-address",
                        format!("invalid target for forward \"{name}\": {e}"),
                    )
                    .at(spans.value(&path("target")))
                    .label("expected host:port or a port")
                    .help("use host:port as seen from the client, for example 127.0.0.1:22, or a port alone like 22"),
                );
                return None;
            }
            valid_name(client).then(|| VisitTarget::Client {
                client: client.clone(),
                target,
            })
        }
        (Some(_), _, _) => {
            out.push(problem(
                format!("forward \"{name}\" has both \"service\" and \"client\" or \"target\""),
                "use one or the other",
                both,
            ));
            None
        }
        (None, Some(_), None) => {
            out.push(problem(
                format!("forward \"{name}\" has \"client\" but no \"target\""),
                "missing target",
                "add the host:port to reach as seen from the client, for example target: 127.0.0.1:22",
            ));
            None
        }
        (None, None, Some(_)) => {
            out.push(problem(
                format!("forward \"{name}\" has \"target\" but no \"client\""),
                "missing client",
                "add the client to reach, for example client: home",
            ));
            None
        }
        (None, None, None) => {
            out.push(problem(
                format!("forward \"{name}\" needs \"service\", or \"client\" and \"target\""),
                "nothing to reach",
                both,
            ));
            None
        }
    }
}
