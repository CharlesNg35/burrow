use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use super::super::settings::{
    ClientAuth, ServerSettings, TlsMaterial, VisitorAuth, WebSocketSettings,
};
use super::super::tree::{Node, Span, Value};
use super::ports::ranges_conflict;
use super::service::{Taken, check_service};
use super::{
    Spans, binds_conflict, name_error, parse_bind, parse_local, secret_error, tuning, valid_name,
};
use crate::allowlist::AllowList;
use crate::base::suggest::{closest, quoted_list};
use crate::diagnostics::Diagnostic;
use crate::noise::MAX_SECRETS_PER_CLIENT;
use crate::secret::Secret;
use burrow_config::SELF;
use burrow_config::{Bind, Protocol, Secrets, ServerConfig, WebSocketConfig};

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
                    .help(format!("use an IP and port, for example 0.0.0.0:{p} or [::]:{p}, or a port alone like {p}", p = burrow_syntax::ports::SERVER_PORT)),
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
                .help(format!("add \"listen: 0.0.0.0:{}\" for TCP clients, or a websocket section for clients behind HTTP-only networks (see https://github.com/charlesng35/burrow/blob/main/docs/websocket.md)", burrow_syntax::ports::SERVER_PORT)),
        );
    }
    let websocket = cfg
        .websocket
        .as_ref()
        .and_then(|ws| check_websocket(ws, listen, spans, base_dir, out));

    if let Some(text) = &cfg.public_address
        && (listen.is_some() || websocket.is_some())
        && let Err(e) = burrow_syntax::address::client_address(
            text,
            listen,
            websocket
                .as_ref()
                .map(crate::config::settings::WebSocketSettings::shape),
            "clients",
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
                crate::base::failure::cmd("gensecret")
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

    super::local::check_reserved(cfg, spans, out);
    let self_allow = super::local::check_allow(cfg, spans, out);

    let mut visitors = BTreeMap::new();
    for (name, entry) in &cfg.visitors {
        if !valid_name(name) {
            out.push(name_error("visitor", name, spans.key(&["visitors", name])));
        }
        let secrets = check_secrets("visitor", name, &entry.secret, spans, &mut seen, out);
        let mut reach = BTreeMap::new();
        for (client, given) in &entry.reach {
            if client == SELF {
                super::local::check_reach(name, self_allow.as_ref(), spans, out);
            }
            if let Some(rules) = check_reach(name, client, given, spans, out) {
                reach.insert(client.clone(), rules);
            }
        }
        for client in reach
            .iter()
            .filter(|(c, r)| {
                (cfg.clients.contains_key(*c) || (*c == SELF && self_allow.is_some()))
                    && **r == AllowList::All
            })
            .map(|(c, _)| c)
        {
            out.push(
                Diagnostic::warning(
                    "reach-any",
                    format!(
                        "visitor \"{name}\" may reach any target that client \"{client}\" allows, over TCP or UDP"
                    ),
                )
                .at(spans.key(&["visitors", name, "reach", client]))
                .label("limited only by the client's allow list")
                .help(format!(
                    "keep {} narrow, for example allow: [127.0.0.1/32:22], or list only the services this visitor needs",
                    super::local::allow_place(client)
                )),
            );
        }
        let listed = cfg.services.values().any(|s| s.visitors.contains(name));
        let admin_from = check_admin(name, entry, listed, spans, out);
        if !listed
            && entry.admin.is_none()
            && entry.reach.is_empty()
            && !spans.has(&["visitors", name, "reach"])
        {
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
                reach,
                max_connections: entry.max_connections.map(|m| m as usize),
                admin: entry.admin,
                admin_from,
            },
        );
    }

    if cfg.services.is_empty() && cfg.visitors.values().all(|v| v.reach.is_empty()) {
        out.push(
            Diagnostic::warning("no-services", "no services are defined, so nothing is exposed")
                .at(spans.key(&["services"]))
                .help(format!("add a service, for example:\n         services:\n           web: {{ client: NAME, bind: 0.0.0.0:{}, target: 127.0.0.1:80 }}", burrow_syntax::ports::FIRST_SERVICE_PORT)),
        );
    }

    let mut services = BTreeMap::new();
    let mut taken = Taken {
        listen,
        websocket: websocket.as_ref().map(|w| w.listen),
        binds: Vec::new(),
    };
    for (name, svc) in &cfg.services {
        if let Some(service) = check_service(name, svc, &mut taken, spans, out) {
            services.insert(name.clone(), service);
        }
    }

    if let Some(ws) = websocket.as_ref().filter(|w| w.listen.ip().is_loopback()) {
        for (name, svc) in &services {
            let by_source = !svc.sources.is_open() || svc.proxy_protocol.is_some();
            if svc.visitors.is_empty() || !by_source {
                continue;
            }
            out.push(
                Diagnostic::warning(
                    "proxied-source",
                    format!(
                        "visitors of service \"{name}\" that connect through the proxy in front of websocket.listen ({}) all come from the proxy's address, so allow_from, deny_from and proxy_protocol see that address for them",
                        ws.listen
                    ),
                )
                .at(spans.key(&["services", name]))
                .help("rely on the visitor's secret for those visitors, or have them connect to the TCP listen port"),
            );
        }
    }

    for name in clients.keys() {
        let list = super::super::settings::services_for(&services, name);
        let (count, size) = (
            list.len(),
            crate::protocol::ServerMsg::Services(list).encode().len(),
        );
        if size > crate::noise::MAX_PLAINTEXT {
            out.push(
                Diagnostic::error(
                    "too-many-services",
                    format!(
                        "client \"{name}\" serves {count} services, too many to send it in one message ({size} bytes, the limit is {})",
                        crate::noise::MAX_PLAINTEXT
                    ),
                )
                .at(spans.key(&["clients", name]))
                .help("spread the services over more clients, or use port ranges for services that differ only by port"),
            );
        }
    }

    let metrics = cfg
        .metrics
        .as_ref()
        .and_then(|m| check_metrics(&m.listen, &taken, spans, out));

    let own: Vec<(SocketAddr, &str)> = [
        (listen, "listen"),
        (websocket.as_ref().map(|w| w.listen), "websocket.listen"),
        (metrics, "metrics.listen"),
    ]
    .into_iter()
    .filter_map(|(a, what)| a.map(|a| (a, what)))
    .collect();
    super::local::check_services(&services, self_allow.as_ref(), &own, spans, out);

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
        udp_max_peers: cfg.udp.max_peers as usize,
        metrics,
        self_allow,
    })
}

fn check_admin(
    name: &str,
    entry: &burrow_config::VisitorEntry,
    listed: bool,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Vec<crate::allowlist::Cidr> {
    let mut nets = Vec::new();
    for (i, text) in entry.admin_from.iter().enumerate() {
        match text.parse::<crate::allowlist::Cidr>() {
            Ok(net) => nets.push(net),
            Err(e) => out.push(
                Diagnostic::error(
                    "invalid-source",
                    format!(
                        "invalid admin_from entry for visitor \"{name}\": {}",
                        e.describe()
                    ),
                )
                .at(spans.value(&["visitors", name, "admin_from", &i.to_string()]))
                .label("expected an IP or CIDR")
                .help(
                    "use an IP like 203.0.113.7 or a network like 203.0.113.0/24 or 2001:db8::/32",
                ),
            ),
        }
    }
    if entry.admin.is_none() && !entry.admin_from.is_empty() {
        out.push(
            Diagnostic::warning(
                "unused-admin-from",
                format!(
                    "visitor \"{name}\" has admin_from but no admin access, so it has no effect"
                ),
            )
            .at(spans.key(&["visitors", name, "admin_from"]))
            .help("add \"admin: read\" or \"admin: write\", or remove admin_from"),
        );
    }
    if entry.admin.is_some() && (listed || !entry.reach.is_empty()) {
        out.push(
            Diagnostic::warning(
                "admin-shared",
                format!(
                    "visitor \"{name}\" has admin access and also reaches services or clients, so one leaked secret gives both"
                ),
            )
            .at(spans.key(&["visitors", name, "admin"]))
            .label("admin access here")
            .help("use a dedicated visitor for burrow admin, and another one for burrow visit"),
        );
    }
    nets
}

fn check_metrics(
    listen: &Bind,
    taken: &Taken<'_>,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Option<SocketAddr> {
    let at = spans.value(&["metrics", "listen"]);
    let addr = match parse_local(listen) {
        Ok((addr, 1)) => addr,
        Ok(_) | Err(_) => {
            out.push(
                Diagnostic::error(
                    "invalid-address",
                    format!("invalid metrics listen address \"{listen}\""),
                )
                .at(at)
                .label("expected ip:port or a port")
                .help("use a port like 9100 (on 127.0.0.1), or ip:port like 127.0.0.1:9100"),
            );
            return None;
        }
    };
    let clash = [
        (taken.listen, "the control port (listen)".to_string()),
        (taken.websocket, "websocket.listen".to_string()),
    ]
    .into_iter()
    .filter_map(|(a, what)| a.map(|a| (a, 1, what)))
    .chain(
        taken
            .binds
            .iter()
            .filter(|(_, _, p, _)| *p == Protocol::Tcp)
            .map(|(a, n, _, name)| (*a, *n, format!("service \"{name}\""))),
    )
    .find(|(a, n, _)| ranges_conflict(*a, *n, addr, 1));
    if let Some((_, _, what)) = clash {
        out.push(
            Diagnostic::error(
                "duplicate-bind",
                format!("metrics.listen {addr} is already used by {what}"),
            )
            .at(at)
            .label("address already used")
            .help("pick another port for the metrics endpoint"),
        );
        return None;
    }
    if !addr.ip().is_loopback() {
        out.push(
            Diagnostic::warning(
                "public-metrics",
                format!("the metrics endpoint listens on {addr}, not only on this machine"),
            )
            .at(at)
            .label("reachable from the network")
            .help("anyone who reaches it sees client, service and visitor names and traffic; use 127.0.0.1 unless a firewall limits who can connect"),
        );
    }
    Some(addr)
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
                crate::base::failure::cmd("gensecret")
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
                                crate::base::failure::cmd("gensecret")
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
                .help(format!("use an IP and port, for example 127.0.0.1:{} behind a proxy, or a port alone for 0.0.0.0", burrow_syntax::ports::WS_PORT)),
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
    if !burrow_syntax::endpoint::valid_path(&ws.path) {
        out.push(
            Diagnostic::error(
                "invalid-path",
                format!("invalid websocket path \"{}\"", ws.path),
            )
            .at(spans.value(&["websocket", "path"]))
            .label("expected a path like /burrow")
            .help("start the path with \"/\" and use letters, digits and - . _ ~ / only, at most 1024 of them"),
        );
    }
    let tls = ws.tls.as_ref().and_then(|t| {
        let read = |file: &str, field: &'static str, out: &mut Vec<Diagnostic>| {
            let path = base_dir.join(file);
            match std::fs::read(&path) {
                Ok(bytes) => Some((path, bytes)),
                Err(e) => {
                    out.push(
                        Diagnostic::error("tls-unreadable", format!("cannot read websocket.tls.{field} \"{file}\": {}", crate::base::failure::file_reason(&e)))
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
                && name != SELF
                && !clients.contains(&name)
            {
                out.push(unknown_ref("client", name, &clients, by(), client.span));
            }
            if let Some(Value::Seq(items)) = svc.value.get("clients").map(|v| &v.value) {
                for item in items {
                    if let Some(name) = item.as_str()
                        && name != SELF
                        && !clients.contains(&name)
                    {
                        out.push(unknown_ref("client", name, &clients, by(), item.span));
                    }
                }
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
                if r.key != SELF && !clients.contains(&r.key.as_str()) {
                    let by = format!("\"reach\" of visitor \"{}\"", visitor.key);
                    out.push(unknown_ref("client", &r.key, &clients, by, r.key_span));
                }
            }
        }
    }
}

fn check_reach(
    visitor: &str,
    client: &str,
    given: &burrow_config::Reach,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Option<AllowList> {
    let entries = given.entries();
    let many = matches!(given, burrow_config::Reach::Many(_));
    let at = |i: usize| {
        let path = ["visitors", visitor, "reach", client];
        if many {
            spans.value(&[&path[..], &[i.to_string().as_str()]].concat())
        } else {
            spans.value(&path)
        }
    };
    if let [one] = entries
        && !many
        && one.trim() == "any"
    {
        return Some(AllowList::All);
    }
    if let [one] = entries
        && !many
        && one.chars().all(|c| c.is_ascii_alphabetic())
    {
        out.push(
            Diagnostic::error(
                "invalid-reach",
                format!("invalid reach \"{one}\" for client \"{client}\""),
            )
            .at(at(0))
            .label("expected any or IP ranges")
            .help(format!(
                "write {client}: any for any target its allow list permits, or narrow it with ranges, for example {client}: [192.168.1.0/24]"
            )),
        );
        return None;
    }
    if entries.is_empty() {
        out.push(
            Diagnostic::error(
                "invalid-reach",
                format!("reach for client \"{client}\" of visitor \"{visitor}\" is an empty list, which lets it reach nothing"),
            )
            .at(spans.value(&["visitors", visitor, "reach", client]))
            .help(format!("remove \"{client}\" from reach, or list what it may reach, for example {client}: [192.168.1.0/24]")),
        );
        return None;
    }
    if entries.len() > crate::protocol::MAX_ALLOW_RULES {
        out.push(
            Diagnostic::error(
                "invalid-reach",
                format!(
                    "reach for client \"{client}\" of visitor \"{visitor}\" has {} rules, the limit is {}",
                    entries.len(),
                    crate::protocol::MAX_ALLOW_RULES
                ),
            )
            .at(spans.value(&["visitors", visitor, "reach", client]))
            .help("combine them into wider ranges, for example 10.0.0.0/8 instead of each /24"),
        );
        return None;
    }
    let mut rules = Vec::new();
    let mut ok = true;
    for (i, entry) in entries.iter().enumerate() {
        if entry.trim() == "any" {
            out.push(
                Diagnostic::error(
                    "invalid-reach",
                    "\"any\" cannot be combined with other rules",
                )
                .at(at(i))
                .help(format!("write {client}: any alone, or list only rules")),
            );
            ok = false;
            continue;
        }
        match super::client::parse_rule(entry, "reach", at(i), out) {
            Some(r) => rules.push(r),
            None => ok = false,
        }
    }
    ok.then_some(AllowList::Rules(rules))
}
