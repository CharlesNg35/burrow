use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::time::Duration;

use super::model::{Bind, ClientConfig, Protocol, ServerConfig, TcpOptions, WebSocketConfig};
use super::settings::{
    ClientAuth, ClientSettings, ServerSettings, Service, SocketTuning, TlsMaterial,
    WebSocketSettings,
};
use super::tree::{Node, Span, Value};
use crate::allowlist::{AllowList, Rule};
use crate::diagnostics::Diagnostic;
use crate::proxy::ProxyConfig;
use crate::secret::{Secret, SecretError};
use crate::suggest::{closest, quoted_list};
use crate::transport::endpoint::Endpoint;

pub const MAX_SECRETS_PER_CLIENT: usize = 4;
pub const MAX_NAME_LEN: usize = 64;

const SENSITIVE_PORTS: &[(u16, &str)] = &[
    (22, "SSH"),
    (23, "telnet"),
    (445, "SMB"),
    (2375, "Docker API"),
    (3306, "MySQL"),
    (3389, "RDP"),
    (5432, "PostgreSQL"),
    (5900, "VNC"),
    (6379, "Redis"),
    (9200, "Elasticsearch"),
    (11211, "memcached"),
    (27017, "MongoDB"),
];

pub struct Spans {
    map: HashMap<Vec<String>, (Span, Span)>,
    root: Span,
}

impl Spans {
    pub fn new(root: &Node) -> Self {
        let mut map = HashMap::new();
        fn walk(n: &Node, path: &mut Vec<String>, map: &mut HashMap<Vec<String>, (Span, Span)>) {
            match &n.value {
                Value::Map(entries) => {
                    for e in entries {
                        path.push(e.key.clone());
                        map.insert(path.clone(), (e.key_span, e.value.span));
                        walk(&e.value, path, map);
                        path.pop();
                    }
                }
                Value::Seq(items) => {
                    for (i, item) in items.iter().enumerate() {
                        path.push(i.to_string());
                        map.insert(path.clone(), (item.span, item.span));
                        walk(item, path, map);
                        path.pop();
                    }
                }
                _ => {}
            }
        }
        walk(root, &mut Vec::new(), &mut map);
        Spans {
            map,
            root: Span::point(root.span.start),
        }
    }

    fn get(&self, path: &[&str]) -> Option<(Span, Span)> {
        let key: Vec<String> = path.iter().map(|s| s.to_string()).collect();
        self.map.get(&key).copied()
    }

    pub fn value(&self, path: &[&str]) -> Span {
        self.get(path).map_or(self.root, |(_, v)| v)
    }

    pub fn key(&self, path: &[&str]) -> Span {
        self.get(path).map_or(self.root, |(k, _)| k)
    }

    pub fn has(&self, path: &[&str]) -> bool {
        self.get(path).is_some()
    }
}

pub fn secret_spans(root: &Node) -> Vec<Span> {
    let mut out = Vec::new();
    fn collect(n: &Node, out: &mut Vec<Span>) {
        match &n.value {
            Value::Seq(items) => items.iter().for_each(|i| collect(i, out)),
            Value::Map(_) => {}
            Value::Str(s) if s.contains("${") => {}
            _ => out.push(n.span),
        }
    }
    if let Some(s) = root.get("secret") {
        collect(s, &mut out);
    }
    if let Some(Value::Map(clients)) = root.get("clients").map(|c| &c.value) {
        for c in clients {
            if let Some(s) = c.value.get("secret") {
                collect(s, &mut out);
            }
        }
    }
    out
}

pub fn parse_host_port(s: &str) -> Result<(String, u16), String> {
    let (host, port) = if let Some(rest) = s.strip_prefix('[') {
        let (h, tail) = rest
            .split_once(']')
            .ok_or_else(|| format!("\"{s}\" has an unclosed \"[\""))?;
        let p = tail
            .strip_prefix(':')
            .ok_or_else(|| format!("\"{s}\" is missing a port"))?;
        (h, p)
    } else {
        match s.rsplit_once(':') {
            Some((h, _)) if h.contains(':') => {
                return Err(format!(
                    "\"{s}\" looks like an IPv6 address; write it as [addr]:port"
                ));
            }
            Some((h, p)) => (h, p),
            None => return Err(format!("\"{s}\" is missing a port")),
        }
    };
    if host.is_empty() {
        return Err(format!("\"{s}\" is missing a host"));
    }
    if host.len() > 253
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
    {
        return Err(format!("\"{host}\" is not a valid host name or IP address"));
    }
    let port: u16 = port
        .parse()
        .map_err(|_| format!("\"{port}\" is not a valid port (1-65535)"))?;
    if port == 0 {
        return Err("port 0 is not allowed".into());
    }
    Ok((host.to_string(), port))
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_LEN
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn name_error(kind: &str, name: &str, span: Span) -> Diagnostic {
    Diagnostic::error("invalid-name", format!("invalid {kind} name \"{name}\""))
        .at(span)
        .label("not a valid name")
        .help(format!(
            "use 1-{MAX_NAME_LEN} letters, digits, \"-\", \"_\" or \".\""
        ))
}

fn secret_error(owner: &str, err: &SecretError, span: Span) -> Diagnostic {
    let help = match err {
        SecretError::NotDecodable { .. } => {
            "use a 32-byte value encoded as base64 or hex; generate one with: burrow gensecret"
        }
        _ => "generate one with: burrow gensecret",
    };
    Diagnostic::error(
        "weak-secret",
        format!("secret for {owner} {}", err.describe()),
    )
    .at(span)
    .without_snippet()
    .help(help)
}

fn tuning(t: &TcpOptions) -> SocketTuning {
    SocketTuning {
        nodelay: t.nodelay,
        keepalive: (t.keepalive_secs > 0).then(|| Duration::from_secs(t.keepalive_secs.into())),
    }
}

fn binds_conflict(a: SocketAddr, b: SocketAddr) -> bool {
    a.port() == b.port()
        && (a.ip() == b.ip()
            || (a.ip().is_unspecified() && a.is_ipv4() == b.is_ipv4())
            || (b.ip().is_unspecified() && a.is_ipv4() == b.is_ipv4()))
}

fn parse_bind(bind: &Bind) -> Result<SocketAddr, String> {
    let addr = match bind {
        Bind::Port(p) => SocketAddr::from(([0, 0, 0, 0], *p)),
        Bind::Addr(s) => {
            if let Ok(p) = s.parse::<u16>() {
                SocketAddr::from(([0, 0, 0, 0], p))
            } else {
                s.parse::<SocketAddr>()
                    .map_err(|_| format!("\"{s}\" is not a valid bind address"))?
            }
        }
    };
    if addr.port() == 0 {
        return Err("port 0 is not allowed".into());
    }
    Ok(addr)
}

pub fn check_server(
    cfg: &ServerConfig,
    spans: &Spans,
    base_dir: &Path,
    out: &mut Vec<Diagnostic>,
) -> Option<ServerSettings> {
    let errors_before = out.iter().filter(|d| d.is_error()).count();

    let listen = match &cfg.listen {
        None => None,
        Some(text) => match text.parse::<SocketAddr>() {
            Ok(a) if a.port() != 0 => Some(a),
            _ => {
                out.push(
                    Diagnostic::error(
                        "invalid-address",
                        format!("invalid listen address \"{text}\""),
                    )
                    .at(spans.value(&["listen"]))
                    .label("expected ip:port")
                    .help("use an IP and port, for example 0.0.0.0:7835 or [::]:7835"),
                );
                None
            }
        },
    };
    if cfg.listen.is_none() && cfg.websocket.is_none() {
        out.push(
            Diagnostic::error("no-listener", "the server needs \"listen\" (TCP), \"websocket\", or both")
                .help("add \"listen: 0.0.0.0:7835\" for TCP clients, or a websocket section for clients behind HTTP-only networks (see README)"),
        );
    }
    let websocket = cfg
        .websocket
        .as_ref()
        .and_then(|ws| check_websocket(ws, listen, spans, base_dir, out));

    if cfg.clients.is_empty() {
        out.push(
            Diagnostic::warning(
                "no-clients",
                "no clients are defined, so nobody can connect",
            )
            .at(spans.key(&["clients"]))
            .help("add a client with: burrow init server, or burrow gensecret"),
        );
    }

    let mut clients = BTreeMap::new();
    let mut seen_secrets: Vec<([u8; 32], String)> = Vec::new();
    for (name, entry) in &cfg.clients {
        if !valid_name(name) {
            out.push(name_error("client", name, spans.key(&["clients", name])));
        }
        let owner = format!("client \"{name}\"");
        let list = entry.secret.as_slice();
        if list.is_empty() {
            out.push(
                Diagnostic::error(
                    "missing-secret",
                    format!("{owner} has an empty secret list"),
                )
                .at(spans.value(&["clients", name, "secret"]))
                .help("add at least one secret; generate one with: burrow gensecret"),
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
                .at(spans.value(&["clients", name, "secret"]))
                .without_snippet()
                .help("keep only the current and the next secret while rotating"),
            );
        }
        let mut secrets = Vec::new();
        for (i, text) in list.iter().enumerate() {
            let idx = i.to_string();
            let span = match &entry.secret {
                super::model::Secrets::One(_) => spans.value(&["clients", name, "secret"]),
                super::model::Secrets::Many(_) => spans.value(&["clients", name, "secret", &idx]),
            };
            match Secret::parse(text) {
                Ok(s) => {
                    if let Some((_, other)) = seen_secrets.iter().find(|(b, _)| b == s.as_bytes()) {
                        let msg = if other == name {
                            format!("{owner} lists the same secret twice")
                        } else {
                            format!("{owner} uses the same secret as client \"{other}\"")
                        };
                        out.push(
                            Diagnostic::error("duplicate-secret", msg)
                                .at(span)
                                .without_snippet()
                                .help("every client needs its own secret; generate one with: burrow gensecret"),
                        );
                    }
                    seen_secrets.push((*s.as_bytes(), name.clone()));
                    secrets.push(s);
                }
                Err(e) => out.push(secret_error(&owner, &e, span)),
            }
        }
        clients.insert(
            name.clone(),
            ClientAuth {
                secrets,
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
        let bind = match parse_bind(&svc.bind) {
            Ok(b) => Some(b),
            Err(e) => {
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
        if let Err(e) = parse_host_port(&svc.target) {
            out.push(
                Diagnostic::error(
                    "invalid-address",
                    format!("invalid target for service \"{name}\": {e}"),
                )
                .at(spans.value(&path("target")))
                .label("expected host:port")
                .help("use host:port as seen from the client, for example 127.0.0.1:3000"),
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
            services.insert(
                name.clone(),
                Service {
                    client: svc.client.clone(),
                    bind,
                    target: svc.target.clone(),
                    protocol: svc.protocol,
                    max_connections: svc.max_connections.map(|m| m as usize),
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
        websocket,
        clients,
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

fn insecure_warning(spans: &Spans) -> Diagnostic {
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
    let addr = match ws.listen.parse::<SocketAddr>() {
        Ok(a) if a.port() != 0 => Some(a),
        _ => {
            out.push(
                Diagnostic::error(
                    "invalid-address",
                    format!("invalid websocket listen address \"{}\"", ws.listen),
                )
                .at(spans.value(&["websocket", "listen"]))
                .label("expected ip:port")
                .help("use an IP and port, for example 127.0.0.1:7836 behind a proxy"),
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
                        Diagnostic::error("tls-file", format!("cannot read websocket.tls.{field} \"{}\": {}", path.display(), crate::failure::file_reason(&e)))
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

const RESERVED_HEADERS: &[&str] = &[
    "host",
    "upgrade",
    "connection",
    "content-length",
    "transfer-encoding",
];

fn check_headers(
    cfg: &ClientConfig,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    for (name, value) in &cfg.headers {
        let lower = name.to_ascii_lowercase();
        let reserved =
            RESERVED_HEADERS.contains(&lower.as_str()) || lower.starts_with("sec-websocket-");
        let valid_name = http::HeaderName::try_from(name.as_str()).is_ok();
        if reserved || !valid_name {
            out.push(
                Diagnostic::error("invalid-header", format!("header \"{name}\" cannot be set"))
                    .at(spans.key(&["headers", name]))
                    .label(if reserved { "set by burrow itself" } else { "not a valid header name" })
                    .help("use custom headers such as an access token, for example CF-Access-Client-Id"),
            );
            continue;
        }
        if http::HeaderValue::try_from(value.as_str()).is_err() {
            out.push(
                Diagnostic::error(
                    "invalid-header",
                    format!("header \"{name}\" has an invalid value"),
                )
                .at(spans.value(&["headers", name]))
                .without_snippet()
                .help("header values cannot contain line breaks or control characters"),
            );
            continue;
        }
        headers.push((name.clone(), value.clone()));
    }
    headers
}

pub fn check_client_refs(root: &Node, out: &mut Vec<Diagnostic>) {
    let declared: Vec<&str> = match root.get("clients").map(|c| &c.value) {
        Some(Value::Map(entries)) => entries.iter().map(|e| e.key.as_str()).collect(),
        _ => Vec::new(),
    };
    let Some(Value::Map(services)) = root.get("services").map(|s| &s.value) else {
        return;
    };
    for svc in services {
        let Some(client) = svc.value.get("client") else {
            continue;
        };
        let Some(name) = client.as_str() else {
            continue;
        };
        if declared.contains(&name) {
            continue;
        }
        let defined = if declared.is_empty() {
            "no clients are defined".to_string()
        } else {
            format!("defined clients: {}", quoted_list(declared.iter().copied()))
        };
        let help = match closest(name, declared.iter().copied()) {
            Some(s) => format!("did you mean \"{s}\"? {defined}"),
            None => format!("add it under \"clients:\"; {defined}"),
        };
        out.push(
            Diagnostic::error(
                "unknown-client",
                format!(
                    "unknown client \"{name}\" referenced by service \"{}\"",
                    svc.key
                ),
            )
            .at(client.span)
            .label("no client with this name")
            .help(help),
        );
    }
}

pub fn check_client(
    cfg: &ClientConfig,
    spans: &Spans,
    base_dir: &Path,
    out: &mut Vec<Diagnostic>,
) -> Option<ClientSettings> {
    let errors_before = out.iter().filter(|d| d.is_error()).count();

    let endpoint = match Endpoint::parse(&cfg.server) {
        Ok(e) => Some(e),
        Err(e) => {
            out.push(
                Diagnostic::error("invalid-address", format!("invalid server address: {e}"))
                    .at(spans.value(&["server"]))
                    .label("expected host:port or wss://host/path")
                    .help("use host:port for TCP (myserver.com:7835), or the WebSocket URL (wss://tunnel.example.com/burrow)"),
            );
            None
        }
    };
    let websocket = endpoint.as_ref().is_some_and(Endpoint::is_websocket);
    let headers = check_headers(cfg, spans, out);
    if !cfg.headers.is_empty() && !websocket && endpoint.is_some() {
        out.push(
            Diagnostic::warning(
                "unused-option",
                "\"headers\" are only sent over WebSocket and are ignored for a TCP server address",
            )
            .at(spans.key(&["headers"]))
            .help("remove \"headers\", or use a ws:// or wss:// server address"),
        );
    }
    let tls = matches!(endpoint, Some(Endpoint::WebSocket { tls: true, .. }));
    let ca_pem = match &cfg.ca {
        None => None,
        Some(file) => {
            let path = base_dir.join(file);
            match std::fs::read(&path)
                .map_err(|e| crate::failure::file_reason(&e))
                .and_then(|pem| crate::transport::tls::parse_certs(&pem).map(|_| pem))
            {
                Ok(pem) => {
                    if !tls && endpoint.is_some() {
                        out.push(
                            Diagnostic::warning(
                                "unused-option",
                                "\"ca\" is only used for wss:// server addresses",
                            )
                            .at(spans.key(&["ca"]))
                            .help("remove \"ca\", or use a wss:// server address"),
                        );
                    }
                    Some(pem)
                }
                Err(e) => {
                    out.push(
                        Diagnostic::error(
                            "tls-file",
                            format!("cannot use ca file \"{}\": {e}", path.display()),
                        )
                        .at(spans.value(&["ca"]))
                        .label("not a readable PEM certificate")
                        .help("paths are relative to the config file"),
                    );
                    None
                }
            }
        }
    };
    if cfg.insecure_plaintext && websocket {
        out.push(
            Diagnostic::warning(
                "unused-option",
                "insecure_plaintext does not apply over WebSocket: forwarded data stays encrypted",
            )
            .at(spans.value(&["insecure_plaintext"]))
            .help(
                "it must still match the server setting; remove it on both sides to silence this",
            ),
        );
    }
    if !valid_name(&cfg.name) {
        out.push(name_error("client", &cfg.name, spans.value(&["name"])));
    }
    let secret = match Secret::parse(&cfg.secret) {
        Ok(s) => Some(s),
        Err(e) => {
            out.push(secret_error(
                &format!("client \"{}\"", cfg.name),
                &e,
                spans.value(&["secret"]),
            ));
            None
        }
    };

    let mut rules = Vec::new();
    for (i, entry) in cfg.allow.iter().enumerate() {
        match entry.parse::<Rule>() {
            Ok(r) => rules.push(r),
            Err(e) => {
                let idx = i.to_string();
                let help = if entry.parse::<IpAddr>().is_err()
                    && !entry.contains('/')
                    && entry.chars().any(|c| c.is_ascii_alphabetic())
                    && !entry.contains("::")
                {
                    "allow entries are IP ranges, not host names; resolve the host and add its address, for example 192.168.1.10/32".to_string()
                } else {
                    "use a CIDR like 192.168.1.0/24, optionally with a port (192.168.1.0/24:22) or a range (:8000-8100)".to_string()
                };
                out.push(
                    Diagnostic::error(
                        "invalid-cidr",
                        format!("invalid allow entry: {}", e.describe()),
                    )
                    .at(spans.value(&["allow", &idx]))
                    .label("not a valid CIDR")
                    .help(help),
                );
            }
        }
    }
    if cfg.allow_all {
        out.push(
            Diagnostic::warning(
                "allow-all",
                "allow_all is enabled: the server can make this client connect anywhere",
            )
            .at(spans.value(&["allow_all"]))
            .label("no restriction on targets")
            .help("list only the networks you expose, for example allow: [127.0.0.0/8]"),
        );
        if !cfg.allow.is_empty() {
            out.push(
                Diagnostic::warning(
                    "allow-ignored",
                    "\"allow\" is ignored because allow_all is true",
                )
                .at(spans.key(&["allow"]))
                .help("remove allow_all to enforce the list"),
            );
        }
    } else if cfg.allow.is_empty() {
        let d = Diagnostic::warning(
            "no-allow-list",
            "no allow list: this client will refuse every target",
        )
        .help("add the networks the server may reach, for example: allow: [127.0.0.0/8]");
        out.push(if spans.has(&["allow"]) {
            d.at(spans.key(&["allow"]))
        } else {
            d
        });
    }

    let proxy = match &cfg.proxy {
        Some(p) => match ProxyConfig::parse(p) {
            Ok(p) => Some(p),
            Err(e) => {
                out.push(
                    Diagnostic::error("invalid-proxy", format!("invalid proxy: {e}"))
                        .at(spans.value(&["proxy"]))
                        .without_snippet()
                        .help("use http://host:port or socks5://[user:pass@]host:port"),
                );
                None
            }
        },
        None => None,
    };

    if cfg.insecure_plaintext && !websocket {
        out.push(insecure_warning(spans));
    }

    let initial = Duration::from_millis(cfg.reconnect.initial_delay_ms.into());
    let max = Duration::from_secs(cfg.reconnect.max_delay_secs.into());
    if initial > max {
        out.push(
            Diagnostic::error(
                "invalid-reconnect",
                "reconnect.initial_delay_ms is larger than reconnect.max_delay_secs",
            )
            .at(spans.value(&["reconnect", "initial_delay_ms"]))
            .help("make the initial delay smaller than the maximum delay"),
        );
    }

    if out.iter().filter(|d| d.is_error()).count() > errors_before {
        return None;
    }
    Some(ClientSettings {
        server: cfg.server.clone(),
        endpoint: endpoint?,
        headers,
        ca_pem,
        name: cfg.name.clone(),
        secret: secret?,
        allow: if cfg.allow_all {
            AllowList::All
        } else {
            AllowList::Rules(rules)
        },
        insecure_plaintext: cfg.insecure_plaintext,
        cipher: cfg.cipher,
        proxy,
        reconnect_initial: initial,
        reconnect_max: max,
        give_up_after: (cfg.reconnect.give_up_after_secs > 0)
            .then(|| Duration::from_secs(cfg.reconnect.give_up_after_secs.into())),
        exit_on_rejected: cfg.reconnect.on_rejected == super::model::OnRejected::Exit,
        connect_timeout: Duration::from_secs(cfg.connect_timeout_secs.into()),
        tcp: tuning(&cfg.tcp),
        udp_session_timeout: Duration::from_secs(cfg.udp.session_timeout_secs.into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_port() {
        assert_eq!(parse_host_port("a.b:80").unwrap(), ("a.b".into(), 80));
        assert_eq!(parse_host_port("[::1]:22").unwrap(), ("::1".into(), 22));
        assert!(parse_host_port("::1:22").is_err());
        assert!(parse_host_port("host").is_err());
        assert!(parse_host_port(":80").is_err());
        assert!(parse_host_port("h:0").is_err());
        assert!(parse_host_port("h:99999").is_err());
        assert!(parse_host_port("bad host:1").is_err());
    }

    #[test]
    fn conflicts() {
        let a = |s: &str| s.parse::<SocketAddr>().unwrap();
        assert!(binds_conflict(a("0.0.0.0:80"), a("127.0.0.1:80")));
        assert!(binds_conflict(a("10.0.0.1:80"), a("10.0.0.1:80")));
        assert!(!binds_conflict(a("10.0.0.1:80"), a("10.0.0.2:80")));
        assert!(!binds_conflict(a("0.0.0.0:80"), a("0.0.0.0:81")));
    }
}
