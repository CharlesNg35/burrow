use burrow_admin_proto::codes::{CHALLENGE_DNS, CHALLENGE_PORT, CHALLENGE_WILDCARD};
use burrow_admin_proto::diag::{Diag, Severity};
use burrow_config::{
    DnsEntry, Entry, LetsEncrypt, ServerConfig, ServiceEntry, WebConfig, WebSite, WebSocketConfig,
};

use crate::config::diagnostics::{Diagnostic, mask_text};
use crate::config::tree::{Node, Span, Value};

struct Place {
    path: Vec<String>,
    key: Span,
    value: Span,
}

fn collect(n: &Node, path: &mut Vec<String>, out: &mut Vec<Place>) {
    match &n.value {
        Value::Map(entries) => {
            for e in entries {
                path.push(e.key.clone());
                out.push(Place {
                    path: path.clone(),
                    key: e.key_span,
                    value: e.value.span,
                });
                collect(&e.value, path, out);
                path.pop();
            }
        }
        Value::Seq(items) => {
            for (i, it) in items.iter().enumerate() {
                path.push(i.to_string());
                out.push(Place {
                    path: path.clone(),
                    key: it.span,
                    value: it.span,
                });
                collect(it, path, out);
                path.pop();
            }
        }
        _ => {}
    }
}

fn also(d: &Diagnostic, path: Option<&[String]>) -> Vec<Vec<String>> {
    match d.code {
        burrow_admin_proto::codes::DUPLICATE_BIND => d
            .subjects
            .iter()
            .map(|n| ServiceEntry::BIND.of(n))
            .filter(|p| Some(p.as_slice()) != path)
            .collect(),
        burrow_admin_proto::codes::UNREACHABLE_SERVICE => match path {
            Some([section, name]) if section == ServiceEntry::section() => {
                vec![ServiceEntry::VISITORS.of(name)]
            }
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

fn challenge_of(path: &[String]) -> Option<Vec<String>> {
    match path {
        [section, name, web, ..]
            if section == ServiceEntry::section() && web == ServiceEntry::WEB.key() =>
        {
            Some(
                ServiceEntry::WEB
                    .of(name)
                    .into_iter()
                    .chain([WebSite::CHALLENGE.key().to_string()])
                    .collect(),
            )
        }
        [ws, ..] if ws == ServerConfig::WEBSOCKET.key() => Some(
            ServerConfig::WEBSOCKET
                .then(WebSocketConfig::CHALLENGE)
                .path(),
        ),
        _ => None,
    }
}

fn own_field(d: &Diagnostic, mut path: Vec<String>) -> Vec<String> {
    if matches!(d.code, CHALLENGE_DNS | CHALLENGE_PORT | CHALLENGE_WILDCARD)
        && let Some(at) = challenge_of(&path)
    {
        return at;
    }
    let list = ServerConfig::WEB
        .then(WebConfig::LETSENCRYPT)
        .then(LetsEncrypt::DNS)
        .path();
    let entry = path.len() == list.len() + 1
        && path.starts_with(&list)
        && path.last().is_some_and(|i| i.parse::<usize>().is_ok());
    if d.code == burrow_admin_proto::codes::DNS_DOMAINS && entry {
        path.push(DnsEntry::DOMAINS.key().to_string());
    }
    path
}

pub struct Mapper {
    places: Vec<Place>,
    names: Vec<String>,
}

impl Mapper {
    pub fn new(root: Option<&Node>) -> Self {
        let mut places = Vec::new();
        if let Some(r) = root {
            collect(r, &mut Vec::new(), &mut places);
        }
        Mapper {
            places,
            names: Vec::new(),
        }
    }

    pub fn named(mut self, names: Vec<String>) -> Self {
        self.names = names;
        self
    }

    fn path_at(&self, at: Span) -> Option<Vec<String>> {
        let within = |s: Span| {
            s.file == at.file
                && s.start.offset <= at.start.offset
                && at.start.offset < s.end.offset.max(s.start.offset + 1)
        };
        self.places
            .iter()
            .filter(|p| within(p.key) || within(p.value))
            .max_by_key(|p| p.path.len())
            .map(|p| p.path.clone())
    }

    pub fn convert(&self, d: &Diagnostic) -> Diag {
        let placed = d.span.filter(|s| s.start.line > 0);
        let path = placed
            .and_then(|s| self.path_at(s))
            .map(|p| own_field(d, p));
        let end = placed.filter(|s| s.end.line > 0 && s.end.offset > s.start.offset);
        let also = also(d, path.as_deref());
        Diag {
            severity: match d.severity {
                crate::config::diagnostics::Severity::Error => Severity::Error,
                crate::config::diagnostics::Severity::Warning => Severity::Warning,
            },
            code: d.code.to_string(),
            message: mask_text(&d.message),
            path,
            also,
            label: d.label.as_deref().map(mask_text),
            help: d.help.as_deref().map(mask_text),
            line: placed.map(|s| s.start.line as u32),
            col: placed.map(|s| s.start.col as u32),
            end_line: end.map(|s| s.end.line as u32),
            end_col: end.map(|s| s.end.col as u32),
            before: false,
            subjects: d.subjects.clone(),
            file: d
                .span
                .filter(|s| s.file != crate::config::tree::FileId::MAIN)
                .and_then(|s| self.names.get(s.file.index()).cloned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_diagnostic_lands_on_the_deepest_field() {
        let text = "services:\n  web:\n    bind: nonsense\n    target: 3000\n";
        let root = crate::config::tree::parse(text).unwrap();
        let m = Mapper::new(Some(&root));
        let at = |word: &str| {
            let offset = text.find(word).unwrap();
            Span::point(crate::config::tree::Pos {
                offset,
                line: 1,
                col: 1,
            })
        };
        assert_eq!(
            m.path_at(at("nonsense")).unwrap(),
            ["services", "web", "bind"]
        );
        assert_eq!(m.path_at(at("web")).unwrap(), ["services", "web"]);
    }

    #[test]
    fn a_diagnostic_over_a_value_says_where_it_ends() {
        let text = "services:\n  web:\n    bind: nonsense\n";
        let root = crate::config::tree::parse(text).unwrap();
        let value = root
            .get("services")
            .unwrap()
            .get("web")
            .unwrap()
            .get("bind")
            .unwrap();
        let m = Mapper::new(Some(&root));
        let mut d = Diagnostic::error("x", "not an address");
        d.span = Some(value.span);
        let shown = m.convert(&d);
        assert_eq!((shown.line, shown.col), (Some(3), Some(11)));
        assert_eq!((shown.end_line, shown.end_col), (Some(3), Some(19)));
        d.span = Some(Span::point(value.span.start));
        let point = m.convert(&d);
        assert_eq!((point.end_line, point.end_col), (None, None));
    }

    #[test]
    fn a_new_dns_entry_without_domains_is_told_at_its_domains() {
        let text = "web:\n  letsencrypt:\n    email: a@example.com\n    dns:\n      - {}\n";
        let root = crate::config::tree::parse(text).unwrap();
        let m = Mapper::new(Some(&root));
        let at = text.find("{}").unwrap();
        let mut d = Diagnostic::error(burrow_admin_proto::codes::DNS_DOMAINS, "domains: list them");
        let pos = |offset: usize| crate::config::tree::Pos {
            offset,
            line: 5,
            col: offset,
        };
        d.span = Some(Span {
            start: pos(at),
            end: pos(at + 2),
            file: crate::config::tree::FileId::MAIN,
        });
        let shown = m.convert(&d);
        assert_eq!(
            shown.path.unwrap(),
            ["web", "letsencrypt", "dns", "0", "domains"]
        );
        let mut other = Diagnostic::error("dns-provider", "name one provider");
        other.span = d.span;
        assert_eq!(
            m.convert(&other).path.unwrap(),
            ["web", "letsencrypt", "dns", "0"]
        );
    }
}
