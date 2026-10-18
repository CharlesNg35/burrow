mod client;
mod connection;
pub mod ports;
mod server;
mod service;
pub mod unix;
mod visitor;

pub use client::check_client;
pub use server::{check_refs, check_server};
pub use visitor::check_visitor;

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::time::Duration;

use super::model::{Bind, TcpOptions};
use super::settings::SocketTuning;
use super::tree::{Node, Span, Value};
use crate::diagnostics::Diagnostic;
use crate::secret::SecretError;

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
    nulls: HashSet<Vec<String>>,
    root: Span,
}

impl Spans {
    pub fn new(root: &Node) -> Self {
        let mut spans = Spans {
            map: HashMap::new(),
            nulls: HashSet::new(),
            root: Span::point(root.span.start),
        };
        fn walk(n: &Node, path: &mut Vec<String>, spans: &mut Spans) {
            match &n.value {
                Value::Map(entries) => {
                    for e in entries {
                        path.push(e.key.clone());
                        spans.map.insert(path.clone(), (e.key_span, e.value.span));
                        if e.value.value == Value::Null {
                            spans.nulls.insert(path.clone());
                        }
                        walk(&e.value, path, spans);
                        path.pop();
                    }
                }
                Value::Seq(items) => {
                    for (i, item) in items.iter().enumerate() {
                        path.push(i.to_string());
                        spans.map.insert(path.clone(), (item.span, item.span));
                        walk(item, path, spans);
                        path.pop();
                    }
                }
                _ => {}
            }
        }
        walk(root, &mut Vec::new(), &mut spans);
        spans
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

    pub fn children(&self, path: &[&str]) -> usize {
        self.map
            .keys()
            .filter(|k| k.len() == path.len() + 1 && k.iter().zip(path).all(|(a, b)| a == b))
            .count()
    }

    pub fn is_null(&self, path: &[&str]) -> bool {
        let key: Vec<String> = path.iter().map(|s| s.to_string()).collect();
        self.nulls.contains(&key)
    }

    pub fn has(&self, path: &[&str]) -> bool {
        self.get(path).is_some()
    }

    pub fn has_value(&self, path: &[&str]) -> bool {
        let key: Vec<String> = path.iter().map(|s| s.to_string()).collect();
        self.has(path) && !self.nulls.contains(&key)
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
    let secretish =
        |key: &str| key == "secret" || crate::suggest::closest(key, ["secret"]).is_some();
    if let Value::Map(top) = &root.value {
        let credentials = |e: &&crate::config::tree::Entry| {
            e.key == "server" && matches!(&e.value.value, Value::Str(s) if s.contains('@'))
        };
        for e in top
            .iter()
            .filter(|e| secretish(&e.key) || e.key == "proxy" || credentials(e))
        {
            collect(&e.value, &mut out);
        }
    }
    if let Some(Value::Map(headers)) = root.get("headers").map(|h| &h.value) {
        for h in headers {
            collect(&h.value, &mut out);
        }
    }
    for section in ["clients", "visitors"] {
        if let Some(Value::Map(sections)) = root.get(section).map(|c| &c.value) {
            for c in sections {
                if let Value::Map(fields) = &c.value.value {
                    for f in fields.iter().filter(|f| secretish(&f.key)) {
                        collect(&f.value, &mut out);
                    }
                }
            }
        }
    }
    out
}

pub const MAX_TARGET_LEN: usize = 255;

pub fn parse_host_port(s: &str) -> Result<(String, u16), String> {
    let (host, port) = if let Some(rest) = s.strip_prefix('[') {
        let (h, tail) = rest
            .split_once(']')
            .ok_or_else(|| format!("\"{s}\" has an unclosed \"[\""))?;
        let p = tail
            .strip_prefix(':')
            .ok_or_else(|| format!("\"{s}\" is missing a port"))?;
        if h.parse::<std::net::Ipv6Addr>().is_err() {
            return Err(format!(
                "only an IPv6 address goes in brackets; write {h}:{p} instead of \"{s}\""
            ));
        }
        (h, p)
    } else {
        match s.rsplit_once(':') {
            Some((h, _)) if h.contains(':') => {
                return Err(format!(
                    "\"{s}\" looks like an IPv6 address; write it in brackets, like [::1]:22"
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
    let make = crate::failure::cmd("gensecret");
    let help = match err {
        SecretError::NotDecodable { .. } => {
            format!("use a 32-byte value encoded as base64 or hex; generate one with: {make}")
        }
        _ => format!("generate one with: {make}"),
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

pub(crate) fn binds_conflict(a: SocketAddr, b: SocketAddr) -> bool {
    let covers =
        |x: SocketAddr, y: SocketAddr| x.ip().is_unspecified() && (x.is_ipv6() || y.is_ipv4());
    a.port() == b.port()
        && (a.ip().to_canonical() == b.ip().to_canonical() || covers(a, b) || covers(b, a))
}

fn parse_bind(bind: &Bind) -> Result<SocketAddr, String> {
    parse_addr(bind, [0, 0, 0, 0], "bind")
}

pub fn parse_local(local: &Bind) -> Result<(SocketAddr, u16), String> {
    ports::parse_ranged(local, [127, 0, 0, 1], "local")
}

fn parse_addr(bind: &Bind, ip: [u8; 4], what: &str) -> Result<SocketAddr, String> {
    let addr = match bind {
        Bind::Port(p) => SocketAddr::from((ip, *p)),
        Bind::Addr(s) => {
            if let Ok(p) = s.trim().parse::<u16>() {
                SocketAddr::from((ip, p))
            } else {
                s.parse::<SocketAddr>()
                    .map_err(|_| format!("\"{s}\" is not a valid {what} address"))?
            }
        }
    };
    if addr.port() == 0 {
        return Err("port 0 is not allowed".into());
    }
    Ok(addr)
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
        assert!(parse_host_port("[127.0.0.1]:80").is_err());
        assert!(parse_host_port("[localhost]:80").is_err());
    }

    #[test]
    fn a_mapped_ipv4_bind_is_the_same_address_as_plain_ipv4() {
        let at = |s: &str| s.parse::<SocketAddr>().unwrap();
        assert!(binds_conflict(at("[::ffff:127.0.0.1]:4000"), at("127.0.0.1:4000")));
        assert!(!binds_conflict(at("[::ffff:127.0.0.1]:4000"), at("127.0.0.2:4000")));
    }

    #[test]
    fn local_ports_default_to_loopback() {
        let p = |b: Bind| parse_local(&b).map(|(a, _)| a.to_string());
        assert_eq!(p(Bind::Port(5432)).unwrap(), "127.0.0.1:5432");
        assert_eq!(p(Bind::Addr("5432".into())).unwrap(), "127.0.0.1:5432");
        assert_eq!(p(Bind::Addr("0.0.0.0:22".into())).unwrap(), "0.0.0.0:22");
        assert_eq!(p(Bind::Addr("[::1]:22".into())).unwrap(), "[::1]:22");
        assert!(p(Bind::Port(0)).is_err());
        assert!(p(Bind::Addr("localhost:22".into())).is_err());
        assert_eq!(
            parse_bind(&Bind::Port(80)).unwrap().to_string(),
            "0.0.0.0:80"
        );
    }

    #[test]
    fn conflicts() {
        let a = |s: &str| s.parse::<SocketAddr>().unwrap();
        assert!(binds_conflict(a("0.0.0.0:80"), a("127.0.0.1:80")));
        assert!(binds_conflict(a("10.0.0.1:80"), a("10.0.0.1:80")));
        assert!(!binds_conflict(a("10.0.0.1:80"), a("10.0.0.2:80")));
        assert!(!binds_conflict(a("0.0.0.0:80"), a("0.0.0.0:81")));
        assert!(binds_conflict(a("0.0.0.0:80"), a("[::]:80")));
        assert!(binds_conflict(a("[::]:80"), a("127.0.0.1:80")));
        assert!(binds_conflict(a("[::1]:80"), a("[::]:80")));
        assert!(!binds_conflict(a("0.0.0.0:80"), a("[::1]:80")));
        assert!(!binds_conflict(a("127.0.0.1:80"), a("[::1]:80")));
    }
}
