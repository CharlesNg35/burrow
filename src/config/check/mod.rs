mod client;
mod connection;
mod entries;
mod local;
pub mod ports;
mod server;
mod service;
pub mod unix;
mod visitor;
mod web;

#[cfg(test)]
mod tests_codes;

pub use client::check_client;
pub use server::{check_refs, check_server};
pub use visitor::check_visitor;

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::time::Duration;

use super::tree::{Node, Span, Value};
use crate::config::diagnostics::Diagnostic;
use crate::wire::secret::SecretError;
use crate::wire::transport::tcp::SocketTuning;
use burrow_config::field::Field;
use burrow_config::{Bind, TcpOptions};

pub use burrow_syntax::hostport::{MAX_TARGET_LEN, parse_host_port};
pub use burrow_syntax::name::{MAX_NAME_LEN, valid_name};
use burrow_syntax::ports::SENSITIVE_PORTS;
pub(crate) use burrow_syntax::ports::binds_conflict;

pub struct Spans {
    map: HashMap<Vec<String>, (Span, Span)>,
    nulls: HashSet<Vec<String>>,
    env: HashSet<Vec<String>>,
    written: Vec<(Vec<String>, String, String)>,
    root: Span,
}

impl Spans {
    pub fn new(root: &Node) -> Self {
        let mut spans = Spans {
            map: HashMap::new(),
            nulls: HashSet::new(),
            env: HashSet::new(),
            written: Vec::new(),
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
                        if e.value.env != super::tree::FromEnv::No {
                            spans.env.insert(path.clone());
                        }
                        if let (Some(raw), Value::Str(now)) = (&e.value.raw, &e.value.value) {
                            spans.written.push((path.clone(), now.clone(), raw.clone()));
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

    pub fn written(&self) -> &[(Vec<String>, String, String)] {
        &self.written
    }

    pub fn from_env(&self, path: &[&str]) -> bool {
        let key: Vec<String> = path.iter().map(|s| s.to_string()).collect();
        self.env.contains(&key)
    }

    pub fn has(&self, path: &[&str]) -> bool {
        self.get(path).is_some()
    }

    pub fn at<P, T>(&self, prefix: &[&str], f: Field<P, T>) -> Span {
        self.value(&joined(prefix, &f))
    }

    pub fn key_at<P, T>(&self, prefix: &[&str], f: Field<P, T>) -> Span {
        self.key(&joined(prefix, &f))
    }

    pub fn has_at<P, T>(&self, prefix: &[&str], f: Field<P, T>) -> bool {
        self.has(&joined(prefix, &f))
    }

    pub fn has_value(&self, path: &[&str]) -> bool {
        let key: Vec<String> = path.iter().map(|s| s.to_string()).collect();
        self.has(path) && !self.nulls.contains(&key)
    }
}

fn joined<'a, P, T>(prefix: &[&'a str], f: &Field<P, T>) -> Vec<&'a str> {
    let mut path = prefix.to_vec();
    path.extend(f.segments());
    path
}

pub fn secretish(key: &str) -> bool {
    use burrow_syntax::secret_fmt::{SECRET_LIKE, SECRET_WORD};
    let key = key.to_ascii_lowercase();
    key.contains(SECRET_WORD)
        || SECRET_LIKE.contains(&key.as_str())
        || crate::base::suggest::closest(&key, [SECRET_WORD]).is_some()
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
    match root.get("headers") {
        Some(Node {
            value: Value::Map(headers),
            ..
        }) => headers.iter().for_each(|h| collect(&h.value, &mut out)),
        Some(other) => collect(other, &mut out),
        None => {}
    }
    let fields = |n: &Node, out: &mut Vec<Span>| match &n.value {
        Value::Map(fields) => fields
            .iter()
            .filter(|f| secretish(&f.key))
            .for_each(|f| collect(&f.value, out)),
        _ => collect(n, out),
    };
    let dns = root
        .get("web")
        .and_then(|w| w.get("letsencrypt"))
        .and_then(|l| l.get("dns"));
    if let Some(Node {
        value: Value::Seq(entries),
        ..
    }) = dns
    {
        for e in entries {
            if let Value::Map(providers) = &e.value {
                providers
                    .iter()
                    .filter(|p| matches!(p.value.value, Value::Map(_)))
                    .for_each(|p| fields(&p.value, &mut out));
            }
        }
    }
    for section in ["clients", "visitors"] {
        match root.get(section).map(|c| &c.value) {
            Some(Value::Map(sections)) => sections.iter().for_each(|c| fields(&c.value, &mut out)),
            Some(Value::Seq(items)) => items.iter().for_each(|c| fields(c, &mut out)),
            _ => {}
        }
    }
    out
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
    let make = crate::base::failure::cmd("gensecret");
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

fn parse_bind(bind: &Bind) -> Result<SocketAddr, String> {
    parse_addr(bind, [0, 0, 0, 0], "bind")
}

pub fn source_list(
    owner: &str,
    field: &str,
    list: &[String],
    path: &[&str],
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Vec<crate::policy::allowlist::Cidr> {
    let mut nets = Vec::new();
    for (i, text) in list.iter().enumerate() {
        match text.parse::<crate::policy::allowlist::Cidr>() {
            Ok(net) => nets.push(net),
            Err(e) => {
                let n = i.to_string();
                let mut at = path.to_vec();
                at.push(&n);
                out.push(
                    Diagnostic::error(
                        "invalid-source",
                        format!("invalid {field} entry for {owner}: {}", e.describe()),
                    )
                    .at(spans.value(&at))
                    .label("expected an IP or CIDR")
                    .help(
                        "use an IP like 203.0.113.7 or a network like 203.0.113.0/24 or 2001:db8::/32",
                    ),
                );
            }
        }
    }
    nets
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
        assert!(binds_conflict(
            at("[::ffff:127.0.0.1]:4000"),
            at("127.0.0.1:4000")
        ));
        assert!(!binds_conflict(
            at("[::ffff:127.0.0.1]:4000"),
            at("127.0.0.2:4000")
        ));
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
        assert!(binds_conflict(a("0.0.0.0:80"), a("[::ffff:127.0.0.1]:80")));
        assert!(binds_conflict(a("[::ffff:10.0.0.1]:80"), a("10.0.0.1:80")));
        assert!(!binds_conflict(a("[::ffff:10.0.0.1]:80"), a("10.0.0.2:80")));
        assert!(binds_conflict(a("[::]:80"), a("127.0.0.1:80")));
        assert!(binds_conflict(a("[::1]:80"), a("[::]:80")));
        assert!(!binds_conflict(a("0.0.0.0:80"), a("[::1]:80")));
        assert!(!binds_conflict(a("127.0.0.1:80"), a("[::1]:80")));
    }
}
