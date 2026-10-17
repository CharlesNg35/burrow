use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use super::super::model::{Protocol, Secrets, ServerConfig, WebSocketConfig};
use super::super::settings::{
    ClientAuth, ServerSettings, Service, TlsMaterial, VisitorAuth, WebSocketSettings,
};
use super::super::tree::{Node, Span, Value};
use super::{
    MAX_SECRETS_PER_CLIENT, MAX_TARGET_LEN, SENSITIVE_PORTS, Spans, binds_conflict, name_error,
    parse_bind, parse_host_port, secret_error, tuning, valid_name,
};
use crate::diagnostics::Diagnostic;
use crate::secret::Secret;
use crate::suggest::{closest, quoted_list};

pub fn check_server(
    cfg: &ServerConfig,
    spans: &Spans,
    base_dir: &Path,
    out: &mut Vec<Diagnostic>,
) -> Option<ServerSettings> {
    let errors_before = out.iter().filter(|d| d.is_error()).count();

    let listen = match &cfg.listen {
        None => None,
        Some(given) => match parse_bind(given) {
            Ok(a) => Some(a),
            Err(_) => {
                out.push(
                    Diagnostic::error(
                        "invalid-address",
                        format!("invalid listen address \"{given}\""),
                    )
                    .at(spans.value(&["listen"]))
                    .label("expected ip:port or a port")
                    .help("use an IP and port, for example 0.0.0.0:7835 or [::]:7835, or a port alone like 7835"),
                );
                None
            }
        },
    };
    if cfg.listen.is_none()
        && cfg.websocket.is_none()
        && !spans.has(&["listen"])
        && !spans.has(&["websocket"])
    {
        out.push(
            Diagnostic::error("no-listener", "the server needs \"listen\" (TCP), \"websocket\", or both")
                .help("add \"listen: 0.0.0.0:7835\" for TCP clients, or a websocket section for clients behind HTTP-only networks (see README)"),
        );
    }
    let websocket = cfg
        .websocket
        .as_ref()
        .and_then(|ws| check_websocket(ws, listen, spans, base_dir, out));

    if let Some(text) = &cfg.public_address
        && (listen.is_some() || websocket.is_some())
        && let Err(e) = crate::config::address::client_address(
            text,
            listen,
            websocket.as_ref().map(Into::into),
            &mut Vec::new(),
        )
    {
        out.push(
            Diagnostic::error(
                "invalid-address",
                format!("invalid public_address: {}", e.message),
            )
            .at(spans.value(&["public_address"]))
            .label("expected a host, host:port or wss:// URL")
            .help(e.hint),
        );
    }

    if cfg.clients.is_empty() {
        out.push(
            Diagnostic::warning(
                "no-clients",
                "no clients are defined, so nobody can connect",
            )
            .at(spans.key(&["clients"]))
            .help(format!(
                "add one, for example:\n         clients:\n           home: {{ secret: SECRET }}\n       with a SECRET from: {}",
                crate::failure::cmd("gensecret")
            )),
        );
    }

    let mut clients = BTreeMap::new();
    let mut seen = Vec::new();
    for (name, entry) in &cfg.clients {
        if !valid_name(name) {
            out.push(name_error("client", name, spans.key(&["clients", name])));
        }
        let secrets = check_secrets("client", name, &entry.secret, spans, &mut seen, out);
        clients.insert(
            name.clone(),
            ClientAuth {
                secrets,
                max_connections: entry.max_connections.map(|m| m as usize),
            },
        );
    }

    let mut visitors = BTreeMap::new();
    for (name, entry) in &cfg.visitors {
        if !valid_name(name) {
            out.push(name_error("visitor", name, spans.key(&["visitors", name])));
        }
        let secrets = check_secrets("visitor", name, &entry.secret, spans, &mut seen, out);
        for client in entry.reach.keys().filter(|c| cfg.clients.contains_key(*c)) {
            out.push(
                Diagnostic::warning(
                    "reach-any",
                    format!(
                        "visitor \"{name}\" may reach any target that client \"{client}\" allows"
                    ),
                )
                .at(spans.key(&["visitors", name, "reach", client]))
                .label("limited only by the client's allow list")
                .help(format!(
                    "keep \"allow\" on client \"{client}\" narrow, for example allow: [127.0.0.1/32:22], or list only the services this visitor needs"
                )),
            );
        }
        let listed = cfg.services.values().any(|s| s.visitors.contains(name));
        if !listed && entry.reach.is_empty() {
            out.push(
                Diagnostic::warning(
                    "unused-visitor",
                    format!("visitor \"{name}\" cannot reach anything"),
                )
                .at(spans.key(&["visitors", name]))
                .help(format!(
                    "add it to a service, for example visitors: [{name}], or give it reach: {{ CLIENT: any }}"
                )),
            );
        }
        visitors.insert(
            name.clone(),
            VisitorAuth {
                secrets,
                reach: entry.reach.keys().cloned().collect(),
                max_connections: entry.max_connections.map(|m| m as usize),
            },
        );
    }

    if cfg.services.is_empty() {
        out.push(
            Diagnostic::warning("no-services", "no services are defined, so nothing is exposed")
                .at(spans.key(&["services"]))
                .help("add a service, for example:\n         services:\n           web: { client: NAME, bind: 0.0.0.0:7840, target: 127.0.0.1:80 }"),
        );
    }

    let mut services = BTreeMap::new();
    let mut binds: Vec<(SocketAddr, Protocol, &str)> = Vec::new();
    for (name, svc) in &cfg.services {
        let path = |f: &'static str| ["services", name.as_str(), f];
        if !valid_name(name) {
            out.push(name_error("service", name, spans.key(&["services", name])));
        }
        let bind = match svc.bind.as_ref().map(parse_bind) {
            None => None,
            Some(Ok(b)) => Some(b),
            Some(Err(e)) => {
                out.push(
                    Diagnostic::error(
                        "invalid-address",
                        format!("invalid bind address for service \"{name}\": {e}"),
                    )
                    .at(spans.value(&path("bind")))
                    .label("expected a port or ip:port")
                    .help("use a port like 7840, or ip:port like 127.0.0.1:7840"),
                );
                None
            }
        };
        if svc.bind.is_none() && svc.visitors.is_empty() && !spans.has(&path("bind")) {
            out.push(
                Diagnostic::error(
                    "unreachable-service",
                    format!("service \"{name}\" has no \"bind\" and no \"visitors\", so nobody can reach it"),
                )
                .at(spans.key(&["services", name]))
                .label("no bind and no visitors")
                .help("add \"bind: 7840\" to expose it publicly, or \"visitors: [NAME]\" for visitors using burrow visit"),
            );
        }
        if !svc.visitors.is_empty() && svc.protocol != Protocol::Tcp {
            out.push(
                Diagnostic::error(
                    "visitors-tcp-only",
                    format!(
                        "service \"{name}\" is {}, but visitors can only reach tcp services",
                        svc.protocol.as_str()
                    ),
                )
                .at(spans.value(&path("visitors")))
                .help("remove \"visitors\", or use a tcp service"),
            );
        }
        let target = svc.target.host_port();
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
                    format!("invalid target for service \"{name}\": {e}"),
                )
                .at(spans.value(&path("target")))
                .label("expected host:port or a port")
                .help("use host:port as seen from the client, for example 127.0.0.1:3000, or a port alone like 3000"),
            );
        }
        if let Some(bind) = bind {
            if let Some((_, _, other)) = binds
                .iter()
                .find(|(b, p, _)| *p == svc.protocol && binds_conflict(*b, bind))
            {
                out.push(
                    Diagnostic::error(
                        "duplicate-bind",
                        format!("service \"{name}\" binds {} {bind}, already used by service \"{other}\"", svc.protocol.as_str()),
                    )
                    .at(spans.value(&path("bind")))
                    .label("address already used")
                    .help("pick a different port for one of the services"),
                );
            }
            if let Some(ws) = &websocket
                && svc.protocol == Protocol::Tcp
                && binds_conflict(ws.listen, bind)
            {
                out.push(
                    Diagnostic::error(
                        "bind-is-control-port",
                        format!("service \"{name}\" binds {bind}, which is the websocket listener"),
                    )
                    .at(spans.value(&path("bind")))
                    .label("same as \"websocket.listen\"")
                    .help(format!("pick a port other than {}", ws.listen.port())),
                );
            }
            if let Some(listen) = listen
                && svc.protocol == Protocol::Tcp
                && binds_conflict(listen, bind)
            {
                out.push(
                    Diagnostic::error(
                        "bind-is-control-port",
                        format!("service \"{name}\" binds {bind}, which is the control port"),
                    )
                    .at(spans.value(&path("bind")))
                    .label("same as \"listen\"")
                    .help(format!("pick a port other than {}", listen.port())),
                );
            }
            if bind.ip().is_unspecified()
                && let Some((_, what)) = SENSITIVE_PORTS.iter().find(|(p, _)| *p == bind.port())
            {
                out.push(
                    Diagnostic::warning(
                        "sensitive-port",
                        format!(
                            "service \"{name}\" exposes port {} ({what}) on all interfaces",
                            bind.port()
                        ),
                    )
                    .at(spans.value(&path("bind")))
                    .label("reachable from the internet")
                    .help(format!(
                        "bind to 127.0.0.1:{} unless it must be public",
                        bind.port()
                    )),
                );
            }
            binds.push((bind, svc.protocol, name));
        }
        let mut listed = Vec::new();
        for v in &svc.visitors {
            if !listed.contains(v) {
                listed.push(v.clone());
            }
        }
        if bind.is_some() || svc.bind.is_none() {
            services.insert(
                name.clone(),
                Service {
                    client: svc.client.clone(),
                    bind,
                    target,
                    protocol: svc.protocol,
                    max_connections: svc.max_connections.map(|m| m as usize),
                    visitors: listed,
                },
            );
        }
    }

    if cfg.pool.min_idle > cfg.pool.max_idle {
        out.push(
            Diagnostic::error(
                "invalid-pool",
                format!(
                    "pool.min_idle ({}) is larger than pool.max_idle ({})",
                    cfg.pool.min_idle, cfg.pool.max_idle
                ),
            )
            .at(spans.value(&["pool", "min_idle"]))
            .label("larger than max_idle")
            .help("make min_idle less than or equal to max_idle"),
        );
    }

    if cfg.insecure_plaintext {
        out.push(insecure_warning(spans));
    }

    if out.iter().filter(|d| d.is_error()).count() > errors_before {
        return None;
    }
    Some(ServerSettings {
        listen,
        public_address: cfg.public_address.clone(),
        websocket,
        clients,
        visitors,
        services,
        insecure_plaintext: cfg.insecure_plaintext,
        heartbeat_interval: Duration::from_secs(cfg.heartbeat.interval_secs.into()),
        handshake_timeout: Duration::from_secs(cfg.limits.handshake_timeout_secs.into()),
        max_handshakes: cfg.limits.max_handshakes as usize,
        max_connections_per_client: cfg.limits.max_connections_per_client as usize,
        max_pending: cfg.limits.max_pending as usize,
        pool_min_idle: cfg.pool.min_idle as usize,
        pool_max_idle: cfg.pool.max_idle as usize,
        tcp: tuning(&cfg.tcp),
        udp_session_timeout: Duration::from_secs(cfg.udp.session_timeout_secs.into()),
    })
}

fn check_secrets(
    kind: &str,
    name: &str,
    secret: &Secrets,
    spans: &Spans,
    seen: &mut Vec<([u8; 32], String)>,
    out: &mut Vec<Diagnostic>,
) -> Vec<Secret> {
    let section = match kind {
        "client" => "clients",
        _ => "visitors",
    };
    let owner = format!("{kind} \"{name}\"");
    let list = secret.as_slice();
    if list.is_empty() {
        out.push(
            Diagnostic::error(
                "missing-secret",
                format!("{owner} has an empty secret list"),
            )
            .at(spans.value(&[section, name, "secret"]))
            .help(format!(
                "add at least one secret; generate one with: {}",
                crate::failure::cmd("gensecret")
            )),
        );
    }
    if list.len() > MAX_SECRETS_PER_CLIENT {
        out.push(
            Diagnostic::error(
                "too-many-secrets",
                format!(
                    "{owner} has {} secrets, the limit is {MAX_SECRETS_PER_CLIENT}",
                    list.len()
                ),
            )
            .at(spans.value(&[section, name, "secret"]))
            .without_snippet()
            .help("keep only the current and the next secret while rotating"),
        );
    }
    let mut secrets = Vec::new();
    for (i, text) in list.iter().enumerate() {
        let idx = i.to_string();
        let span = match secret {
            Secrets::One(_) => spans.value(&[section, name, "secret"]),
            Secrets::Many(_) => spans.value(&[section, name, "secret", &idx]),
        };
        match Secret::parse(text) {
            Ok(s) => {
                if let Some((_, other)) = seen.iter().find(|(b, _)| b == s.as_bytes()) {
                    let msg = if *other == owner {
                        format!("{owner} lists the same secret twice")
                    } else {
                        format!("{owner} uses the same secret as {other}")
                    };
                    let who = if kind == "client" && other.starts_with("client ") {
                        "client"
                    } else {
                        "client and visitor"
                    };
                    out.push(
                        Diagnostic::error("duplicate-secret", msg)
                            .at(span)
                            .without_snippet()
                            .help(format!(
                                "every {who} needs its own secret; generate one with: {}",
                                crate::failure::cmd("gensecret")
                            )),
                    );
                }
                seen.push((*s.as_bytes(), owner.clone()));
                secrets.push(s);
            }
            Err(e) => out.push(secret_error(&owner, &e, span)),
        }
    }
    secrets
}

pub(super) fn insecure_warning(spans: &Spans) -> Diagnostic {
    Diagnostic::warning(
        "insecure-plaintext",
        "insecure_plaintext is enabled: forwarded data is not encrypted",
    )
    .at(spans.value(&["insecure_plaintext"]))
    .label("traffic can be read and modified on the network")
    .help("remove this line unless the network between client and server is trusted")
}

fn check_websocket(
    ws: &WebSocketConfig,
    listen: Option<SocketAddr>,
    spans: &Spans,
    base_dir: &Path,
    out: &mut Vec<Diagnostic>,
) -> Option<WebSocketSettings> {
    let errors_before = out.iter().filter(|d| d.is_error()).count();
    let addr = match parse_bind(&ws.listen) {
        Ok(a) => Some(a),
        Err(_) => {
            out.push(
                Diagnostic::error(
                    "invalid-address",
                    format!("invalid websocket listen address \"{}\"", ws.listen),
                )
                .at(spans.value(&["websocket", "listen"]))
                .label("expected ip:port or a port")
                .help("use an IP and port, for example 127.0.0.1:7836 behind a proxy, or a port alone for 0.0.0.0"),
            );
            None
        }
    };
    if let (Some(a), Some(l)) = (addr, listen)
        && binds_conflict(a, l)
    {
        out.push(
            Diagnostic::error(
                "duplicate-bind",
                format!("websocket.listen {a} is the same address as listen"),
            )
            .at(spans.value(&["websocket", "listen"]))
            .label("already used by \"listen\"")
            .help("use a different port for the websocket listener"),
        );
    }
    if !ws.path.starts_with('/') || ws.path.contains(['?', '#', ' ']) {
        out.push(
            Diagnostic::error(
                "invalid-path",
                format!("invalid websocket path \"{}\"", ws.path),
            )
            .at(spans.value(&["websocket", "path"]))
            .label("expected a path like /burrow")
            .help("start the path with \"/\" and leave out spaces, \"?\" and \"#\""),
        );
    }
    let tls = ws.tls.as_ref().and_then(|t| {
        let read = |file: &str, field: &'static str, out: &mut Vec<Diagnostic>| {
            let path = base_dir.join(file);
            match std::fs::read(&path) {
                Ok(bytes) => Some((path, bytes)),
                Err(e) => {
                    out.push(
                        Diagnostic::error("tls-file", format!("cannot read websocket.tls.{field} \"{file}\": {}", crate::failure::file_reason(&e)))
                            .at(spans.value(&["websocket", "tls", field]))
                            .label("file not readable")
                            .help("paths are relative to the config file; check the path and its permissions"),
                    );
                    None
                }
            }
        };
        let cert = read(&t.cert, "cert", out);
        let key = read(&t.key, "key", out);
        let ((cert_path, cert_pem), (key_path, key_pem)) = (cert?, key?);
        if let Err(e) = crate::transport::tls::acceptor(&cert_pem, &key_pem) {
            out.push(
                Diagnostic::error("tls-file", format!("websocket TLS files are not usable: {e}"))
                    .at(spans.value(&["websocket", "tls"]))
                    .without_snippet()
                    .help("use a PEM certificate chain and its matching PEM private key"),
            );
            return None;
        }
        Some(TlsMaterial {
            cert_path,
            key_path,
            cert_pem,
            key_pem: zeroize::Zeroizing::new(key_pem),
        })
    });
    if out.iter().filter(|d| d.is_error()).count() > errors_before {
        return None;
    }
    Some(WebSocketSettings {
        listen: addr?,
        path: ws.path.clone(),
        tls,
        keepalive: Duration::from_secs(ws.keepalive_secs.into()),
    })
}

fn declared<'a>(root: &'a Node, section: &str) -> Vec<&'a str> {
    match root.get(section).map(|c| &c.value) {
        Some(Value::Map(entries)) => entries.iter().map(|e| e.key.as_str()).collect(),
        _ => Vec::new(),
    }
}

fn unknown_ref(kind: &str, name: &str, known: &[&str], by: String, span: Span) -> Diagnostic {
    let section = format!("{kind}s");
    let defined = if known.is_empty() {
        format!("no {section} are defined")
    } else {
        format!("defined {section}: {}", quoted_list(known.iter().copied()))
    };
    let help = match closest(name, known.iter().copied()) {
        Some(s) => format!("did you mean \"{s}\"? {defined}"),
        None => format!("add it under \"{section}:\"; {defined}"),
    };
    let code = if kind == "client" {
        "unknown-client"
    } else {
        "unknown-visitor"
    };
    Diagnostic::error(
        code,
        format!("unknown {kind} \"{name}\" referenced by {by}"),
    )
    .at(span)
    .label(format!("no {kind} with this name"))
    .help(help)
}

pub fn check_refs(root: &Node, out: &mut Vec<Diagnostic>) {
    let clients = declared(root, "clients");
    let visitors = declared(root, "visitors");
    if let Some(Value::Map(services)) = root.get("services").map(|s| &s.value) {
        for svc in services {
            let by = || format!("service \"{}\"", svc.key);
            if let Some(client) = svc.value.get("client")
                && let Some(name) = client.as_str()
                && !clients.contains(&name)
            {
                out.push(unknown_ref("client", name, &clients, by(), client.span));
            }
            if let Some(Value::Seq(items)) = svc.value.get("visitors").map(|v| &v.value) {
                for item in items {
                    if let Some(name) = item.as_str()
                        && !visitors.contains(&name)
                    {
                        out.push(unknown_ref("visitor", name, &visitors, by(), item.span));
                    }
                }
            }
        }
    }
    if let Some(Value::Map(entries)) = root.get("visitors").map(|v| &v.value) {
        for visitor in entries {
            let Some(Value::Map(reach)) = visitor.value.get("reach").map(|r| &r.value) else {
                continue;
            };
            for r in reach {
                if !clients.contains(&r.key.as_str()) {
                    let by = format!("\"reach\" of visitor \"{}\"", visitor.key);
                    out.push(unknown_ref("client", &r.key, &clients, by, r.key_span));
                }
            }
        }
    }
}
