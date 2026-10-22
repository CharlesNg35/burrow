use burrow_admin_proto::diag::{Diag, Severity};
use burrow_config::ServiceEntry;

use crate::config::tree::{Node, Span, Value};
use crate::diagnostics::{Diagnostic, mask_text};

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

fn quoted_names(text: &str, after: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(after) {
        let tail = &rest[at + after.len()..];
        if let Some(q) = tail.strip_prefix('"')
            && let Some(end) = q.find('"')
        {
            out.push(q[..end].to_string());
            rest = &q[end..];
        } else {
            rest = tail;
        }
    }
    out
}

fn also(d: &Diagnostic, path: Option<&[String]>) -> Vec<Vec<String>> {
    match d.code {
        "duplicate-bind" => quoted_names(&d.message, "service ")
            .into_iter()
            .map(|n| ServiceEntry::BIND.of(&n))
            .filter(|p| Some(p.as_slice()) != path)
            .collect(),
        _ => Vec::new(),
    }
}

pub struct Mapper {
    places: Vec<Place>,
}

impl Mapper {
    pub fn new(root: Option<&Node>) -> Self {
        let mut places = Vec::new();
        if let Some(r) = root {
            collect(r, &mut Vec::new(), &mut places);
        }
        Mapper { places }
    }

    fn path_at(&self, at: usize) -> Option<Vec<String>> {
        let within = |s: Span| s.start.offset <= at && at < s.end.offset.max(s.start.offset + 1);
        self.places
            .iter()
            .filter(|p| within(p.key) || within(p.value))
            .max_by_key(|p| p.path.len())
            .map(|p| p.path.clone())
    }

    pub fn convert(&self, d: &Diagnostic) -> Diag {
        let path = d
            .span
            .filter(|s| s.start.line > 0)
            .and_then(|s| self.path_at(s.start.offset));
        let also = also(d, path.as_deref());
        Diag {
            severity: match d.severity {
                crate::diagnostics::Severity::Error => Severity::Error,
                crate::diagnostics::Severity::Warning => Severity::Warning,
            },
            code: d.code.to_string(),
            message: mask_text(&d.message),
            path,
            also,
            label: d.label.as_deref().map(mask_text),
            help: d.help.as_deref().map(mask_text),
            line: d
                .span
                .filter(|s| s.start.line > 0)
                .map(|s| s.start.line as u32),
            col: d
                .span
                .filter(|s| s.start.line > 0)
                .map(|s| s.start.col as u32),
            before: false,
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
        let at = text.find("nonsense").unwrap();
        assert_eq!(m.path_at(at).unwrap(), ["services", "web", "bind"]);
        let at = text.find("web").unwrap();
        assert_eq!(m.path_at(at).unwrap(), ["services", "web"]);
    }
}
