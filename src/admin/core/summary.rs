use burrow_admin_proto::plan::{Change, ChangeKind};
use serde_json::Value;

use super::values::secretish;
use crate::config::patch::Rename;

fn short(v: &Value) -> String {
    let text = match v {
        Value::Null => "nothing".to_string(),
        Value::String(s) if s.contains("${") => s.clone(),
        Value::String(s) => s.clone(),
        Value::Array(a) if a.is_empty() => "none".into(),
        Value::Array(a) => a
            .iter()
            .map(|x| match x {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(_) => "settings".into(),
        other => other.to_string(),
    };
    if text.chars().count() > 80 {
        format!("{}…", text.chars().take(79).collect::<String>())
    } else {
        text
    }
}

fn entity(section: &str) -> Option<&'static str> {
    match section {
        "clients" => Some("client"),
        "visitors" => Some("visitor"),
        "services" => Some("service"),
        _ => None,
    }
}

fn secret_count(v: Option<&Value>) -> usize {
    match v {
        Some(Value::Array(a)) => a.len(),
        Some(Value::String(_)) => 1,
        _ => 0,
    }
}

fn fields_changed(
    name: &str,
    kind: &str,
    old: &Value,
    new: &Value,
    path: Vec<String>,
    out: &mut Vec<Change>,
) {
    let empty = serde_json::Map::new();
    let (o, n) = (
        old.as_object().unwrap_or(&empty),
        new.as_object().unwrap_or(&empty),
    );
    let mut parts = Vec::new();
    for (k, nv) in n {
        match o.get(k) {
            Some(ov) if ov == nv => {}
            ov if secretish(k) => {
                let (a, b) = (secret_count(ov), secret_count(Some(nv)));
                parts.push(match b.cmp(&a) {
                    std::cmp::Ordering::Greater => format!("secret added ({b} accepted)"),
                    std::cmp::Ordering::Less => format!("secret removed ({b} left)"),
                    std::cmp::Ordering::Equal => "secret replaced".into(),
                });
            }
            None => parts.push(format!("{k} set to {}", short(nv))),
            Some(ov) => parts.push(format!("{k} {} → {}", short(ov), short(nv))),
        }
    }
    for k in o.keys().filter(|k| !n.contains_key(*k)) {
        parts.push(format!("{k} removed"));
    }
    if !parts.is_empty() {
        let label = kind[..1].to_uppercase() + &kind[1..];
        out.push(Change {
            kind: ChangeKind::Set,
            path,
            summary: format!("{label} {name}: {}", parts.join("; ")),
        });
    }
}

pub fn changes(old: &Value, new: &Value, renames: &[Rename]) -> Vec<Change> {
    let mut out = Vec::new();
    let empty = serde_json::Map::new();
    let (o, n) = (
        old.as_object().unwrap_or(&empty),
        new.as_object().unwrap_or(&empty),
    );
    for r in renames {
        if r.path.len() == 2
            && let Some(kind) = entity(&r.path[0])
        {
            out.push(Change {
                kind: ChangeKind::Rename,
                path: r.path.clone(),
                summary: format!(
                    "{} {} renamed to {}",
                    kind[..1].to_uppercase() + &kind[1..],
                    r.path[1],
                    r.to
                ),
            });
        }
    }
    let keys: Vec<&String> = o
        .keys()
        .chain(n.keys().filter(|k| !o.contains_key(*k)))
        .collect();
    for key in keys {
        let (ov, nv) = (o.get(key), n.get(key));
        if ov == nv {
            continue;
        }
        if let Some(kind) = entity(key) {
            let (om, nm) = (
                ov.and_then(Value::as_object).unwrap_or(&empty),
                nv.and_then(Value::as_object).unwrap_or(&empty),
            );
            let renamed_from = |name: &str| {
                renames
                    .iter()
                    .find(|r| r.path.len() == 2 && r.path[0] == *key && r.to == name)
                    .map(|r| r.path[1].clone())
            };
            let renamed_to = |name: &str| {
                renames
                    .iter()
                    .any(|r| r.path.len() == 2 && r.path[0] == *key && r.path[1] == name)
            };
            for (name, val) in nm {
                let path = vec![key.clone(), name.clone()];
                match om
                    .get(name)
                    .or_else(|| renamed_from(name).and_then(|f| om.get(&f)))
                {
                    None => {
                        let extra = match kind {
                            "client" | "visitor" => format!(
                                " ({} {})",
                                secret_count(val.get("secret")),
                                if secret_count(val.get("secret")) == 1 {
                                    "secret"
                                } else {
                                    "secrets"
                                }
                            ),
                            _ => String::new(),
                        };
                        out.push(Change {
                            kind: ChangeKind::Add,
                            path,
                            summary: format!("New {kind} {name}{extra}"),
                        })
                    }
                    Some(before) if before != val => {
                        fields_changed(name, kind, before, val, path, &mut out)
                    }
                    _ => {}
                }
            }
            for name in om.keys().filter(|k| !nm.contains_key(*k) && !renamed_to(k)) {
                out.push(Change {
                    kind: ChangeKind::Remove,
                    path: vec![key.clone(), name.clone()],
                    summary: format!("{} {name} removed", kind[..1].to_uppercase() + &kind[1..]),
                });
            }
            continue;
        }
        let path = vec![key.clone()];
        let summary = match (ov, nv) {
            (Some(Value::Object(a)), Some(Value::Object(b))) => {
                let mut parts = Vec::new();
                for (k, v) in b {
                    if a.get(k) != Some(v) {
                        parts.push(format!(
                            "{key}.{k}: {} → {}",
                            a.get(k).map_or("default".into(), short),
                            short(v)
                        ));
                    }
                }
                for k in a.keys().filter(|k| !b.contains_key(*k)) {
                    parts.push(format!("{key}.{k} back to its default"));
                }
                parts.join("; ")
            }
            (None, Some(v)) => format!("{key} set to {}", short(v)),
            (Some(_), None) => format!("{key} removed"),
            (Some(a), Some(b)) => format!("{key}: {} → {}", short(a), short(b)),
            (None, None) => continue,
        };
        out.push(Change {
            kind: match (ov, nv) {
                (None, _) => ChangeKind::Add,
                (_, None) => ChangeKind::Remove,
                _ => ChangeKind::Set,
            },
            path,
            summary,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn changes_read_like_sentences() {
        let old = json!({"listen": 7835, "clients": {"home": {"secret": "SECRET-A"}}, "services": {"web": {"client": "home", "bind": 7840, "target": 3000}}, "limits": {"max_pending": 9}});
        let new = json!({"listen": 7836, "clients": {"home": {"secret": ["SECRET-A", "SECRET-B"]}, "office": {"secret": "SECRET-C"}}, "services": {"web": {"client": "home", "bind": 7841, "target": 3000}}});
        let c = changes(&old, &new, &[]);
        let text: Vec<&str> = c.iter().map(|c| c.summary.as_str()).collect();
        assert!(text.contains(&"listen: 7835 → 7836"), "{text:?}");
        assert!(
            text.contains(&"Client home: secret added (2 accepted)"),
            "{text:?}"
        );
        assert!(text.contains(&"New client office (1 secret)"), "{text:?}");
        assert!(text.contains(&"Service web: bind 7840 → 7841"), "{text:?}");
        assert!(text.contains(&"limits removed"), "{text:?}");
        assert!(!text.iter().any(|t| t.contains("SECRET-")), "{text:?}");
    }
}
