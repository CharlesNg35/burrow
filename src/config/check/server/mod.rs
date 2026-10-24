mod refs;
mod secrets;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use super::super::settings::{ClientAuth, ServerSettings, VisitorAuth};
use super::ports::ranges_conflict;
use super::service::{Taken, check_service};
use super::{Spans, name_error, parse_bind, parse_local, tuning, valid_name};
use crate::config::diagnostics::Diagnostic;
use crate::policy::allowlist::AllowList;
use burrow_config::SELF;
use burrow_config::{Bind, Protocol, ServerConfig};
use refs::check_reach;
pub use refs::check_refs;
use secrets::{check_secrets, check_websocket};

pub fn check_server(
    cfg: &ServerConfig,
    spans: &Spans,
    base_dir: &Path,
    out: &mut Vec<Diagnostic>,
) -> Option<ServerSettings> {
    let errors_before = out.iter().filter(|d| d.is_error()).count();

    let listen = check_listen(cfg, spans, out);
    let https = super::web::check_https(cfg, listen, spans, out);
    let http = super::web::check_http(cfg, listen, spans, out);
    let mut websocket = cfg
        .websocket
        .as_ref()
        .and_then(|ws| check_websocket(ws, listen, spans, base_dir, out));
    super::web::share_tunnel(cfg, https, websocket.as_mut(), spans, out);

    check_public_address(cfg, listen, websocket.as_ref(), spans, out);

    let mut seen = Vec::new();
    let clients = check_clients(cfg, spans, &mut seen, out);

    super::local::check_reserved(cfg, spans, out);
    let self_allow = super::local::check_allow(cfg, spans, out);

    let visitors = check_visitors(cfg, self_allow.as_ref(), spans, &mut seen, out);

    let (services, taken) = check_all_services(cfg, listen, websocket.as_ref(), https, spans, out);
    let tunnel = websocket.as_ref().and_then(|w| w.host.as_deref());
    let sites = super::web::check_sites(cfg, https, tunnel, spans, base_dir, out);

    warn_proxied_sources(websocket.as_ref(), &services, spans, out);

    check_service_lists(&clients, &services, spans, out);

    let metrics = cfg
        .metrics
        .as_ref()
        .and_then(|m| check_metrics(&m.listen, &taken, spans, out));

    let own: Vec<(SocketAddr, &str)> = [
        (listen, "listen"),
        (websocket.as_ref().map(|w| w.listen), "websocket.listen"),
        (metrics, "metrics.listen"),
        (https, "web.https"),
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
        public_address: cfg
            .public_address
            .clone()
            .or_else(|| websocket.as_ref().and_then(|w| w.shared_url())),
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
        web: super::web::settings(cfg, (https, http), sites, websocket.as_ref(), base_dir),
        websocket,
    })
}

fn check_listen(
    cfg: &ServerConfig,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Option<SocketAddr> {
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
    listen
}

fn check_public_address(
    cfg: &ServerConfig,
    listen: Option<SocketAddr>,
    websocket: Option<&crate::config::settings::WebSocketSettings>,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    if let Some(text) = &cfg.public_address
        && (listen.is_some() || websocket.is_some())
        && let Err(e) = burrow_syntax::address::client_address(
            text,
            listen,
            websocket.map(crate::config::settings::WebSocketSettings::shape),
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
}

fn check_clients(
    cfg: &ServerConfig,
    spans: &Spans,
    seen: &mut Vec<([u8; 32], String)>,
    out: &mut Vec<Diagnostic>,
) -> BTreeMap<String, ClientAuth> {
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
    for (name, entry) in &cfg.clients {
        if !valid_name(name) {
            out.push(name_error("client", name, spans.key(&["clients", name])));
        }
        let secrets = check_secrets("client", name, &entry.secret, spans, seen, out);
        clients.insert(
            name.clone(),
            ClientAuth {
                secrets,
                max_connections: entry.max_connections.map(|m| m as usize),
            },
        );
    }
    clients
}

fn check_visitors(
    cfg: &ServerConfig,
    self_allow: Option<&AllowList>,
    spans: &Spans,
    seen: &mut Vec<([u8; 32], String)>,
    out: &mut Vec<Diagnostic>,
) -> BTreeMap<String, VisitorAuth> {
    let mut visitors = BTreeMap::new();
    for (name, entry) in &cfg.visitors {
        if !valid_name(name) {
            out.push(name_error("visitor", name, spans.key(&["visitors", name])));
        }
        let secrets = check_secrets("visitor", name, &entry.secret, spans, seen, out);
        let mut reach = BTreeMap::new();
        for (client, given) in &entry.reach {
            if client == SELF {
                super::local::check_reach(name, self_allow, spans, out);
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
    visitors
}

fn check_all_services<'a>(
    cfg: &'a ServerConfig,
    listen: Option<SocketAddr>,
    websocket: Option<&crate::config::settings::WebSocketSettings>,
    https: Option<SocketAddr>,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> (
    BTreeMap<String, crate::config::settings::Service>,
    Taken<'a>,
) {
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
        websocket: websocket.filter(|w| w.host.is_none()).map(|w| w.listen),
        web: https,
        binds: Vec::new(),
    };
    for (name, svc) in &cfg.services {
        if let Some(service) = check_service(name, svc, &mut taken, spans, out) {
            services.insert(name.clone(), service);
        }
    }
    (services, taken)
}

fn warn_proxied_sources(
    websocket: Option<&crate::config::settings::WebSocketSettings>,
    services: &BTreeMap<String, crate::config::settings::Service>,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    if let Some(ws) = websocket.filter(|w| w.listen.ip().is_loopback()) {
        for (name, svc) in services {
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
}

fn check_service_lists(
    clients: &BTreeMap<String, ClientAuth>,
    services: &BTreeMap<String, crate::config::settings::Service>,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    for name in clients.keys() {
        let list = super::super::settings::services_for(services, name);
        let (count, size) = (
            list.len(),
            crate::wire::protocol::ServerMsg::Services(list)
                .encode()
                .len(),
        );
        if size > crate::wire::noise::MAX_PLAINTEXT {
            out.push(
                Diagnostic::error(
                    "too-many-services",
                    format!(
                        "client \"{name}\" serves {count} services, too many to send it in one message ({size} bytes, the limit is {})",
                        crate::wire::noise::MAX_PLAINTEXT
                    ),
                )
                .at(spans.key(&["clients", name]))
                .help("spread the services over more clients, or use port ranges for services that differ only by port"),
            );
        }
    }
}

fn check_admin(
    name: &str,
    entry: &burrow_config::VisitorEntry,
    listed: bool,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Vec<crate::policy::allowlist::Cidr> {
    let mut nets = Vec::new();
    for (i, text) in entry.admin_from.iter().enumerate() {
        match text.parse::<crate::policy::allowlist::Cidr>() {
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
        (taken.web, "web.https".to_string()),
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

pub(super) fn insecure_warning(spans: &Spans) -> Diagnostic {
    Diagnostic::warning(
        "insecure-plaintext",
        "insecure_plaintext is enabled: forwarded data is not encrypted",
    )
    .at(spans.value(&["insecure_plaintext"]))
    .label("traffic can be read and modified on the network")
    .help("remove this line unless the network between client and server is trusted")
}
