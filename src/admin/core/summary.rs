use burrow_admin_proto::plan::{Change, ChangeKind};
use burrow_config::{ClientEntry, Entry, ServiceEntry, VisitorEntry};
use serde_json::Value;

use crate::config::check::secretish;
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
        Value::Object(m) if m.is_empty() => "nothing".into(),
        Value::Object(m) => m
            .iter()
            .map(|(k, x)| match x {
                Value::Object(_) => format!("{k}: {{…}}"),
                _ => format!("{k}: {}", short(x)),
            })
            .collect::<Vec<_>>()
            .join(", "),
        other => other.to_string(),
    };
    let text = super::text::mask_secrets(&text, &[]);
    if text.chars().count() > 80 {
        format!("{}…", text.chars().take(79).collect::<String>())
    } else {
        text
    }
}

fn entity(section: &str) -> Option<&'static str> {
    [
        (ClientEntry::section(), ClientEntry::ENTITY),
        (VisitorEntry::section(), VisitorEntry::ENTITY),
        (ServiceEntry::section(), ServiceEntry::ENTITY),
    ]
    .into_iter()
    .find_map(|(s, e)| (s == section).then_some(e))
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
            Some(Value::Object(om)) if nv.is_object() => {
                let nm = nv.as_object().unwrap_or(&empty);
                let mut what: Vec<String> = Vec::new();
                for (sub, x) in nm {
                    match om.get(sub) {
                        None => what.push(format!("{sub} added ({})", short(x))),
                        Some(y) if y != x => {
                            what.push(format!("{sub} {} → {}", short(y), short(x)))
                        }
                        _ => {}
                    }
                }
                what.extend(
                    om.keys()
                        .filter(|sub| !nm.contains_key(*sub))
                        .map(|sub| format!("{sub} removed")),
                );
                parts.push(format!("{k}: {}", what.join(", ")));
            }
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
            part_of: None,
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
                part_of: None,
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
                        let secrets = match kind {
                            k if k == ClientEntry::ENTITY => Some(ClientEntry::SECRET.get(val)),
                            k if k == VisitorEntry::ENTITY => Some(VisitorEntry::SECRET.get(val)),
                            _ => None,
                        };
                        let extra = secrets.map(secret_count).map_or_else(String::new, |n| {
                            format!(" ({n} secret{})", if n == 1 { "" } else { "s" })
                        });
                        out.push(Change {
                            kind: ChangeKind::Add,
                            path,
                            summary: format!("New {kind} {name}{extra}"),
                            part_of: None,
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
                    part_of: None,
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
            (None, Some(Value::Object(b))) if !b.is_empty() => b
                .iter()
                .map(|(k, v)| format!("{key}.{k} set to {}", short(v)))
                .collect::<Vec<_>>()
                .join("; "),
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
            part_of: None,
        });
    }
    let renamed: Vec<(Vec<String>, String)> = renames
        .iter()
        .map(|r| (r.path.clone(), r.to.clone()))
        .collect();
    for c in out.iter_mut().filter(|c| c.kind == ChangeKind::Set) {
        c.part_of = burrow_config::refs::part_of(old, new, &c.path, &renamed);
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
        let old = json!({"visitors": {"lap": {"reach": {"home": "any", "lab": "any"}}}});
        let new = json!({"visitors": {"lap": {"reach": {"home": ["10.0.0.0/8"], "office": "any"}}}, "self": {"allow": ["10.0.0.0/8"]}});
        let c = changes(&old, &new, &[]);
        let text: Vec<&str> = c.iter().map(|c| c.summary.as_str()).collect();
        assert!(
            text.contains(
                &"Visitor lap: reach: home any → 10.0.0.0/8, office added (any), lab removed"
            ),
            "{text:?}"
        );
        assert!(text.contains(&"self.allow set to 10.0.0.0/8"), "{text:?}");
        assert!(!text.iter().any(|t| t.contains("settings")), "{text:?}");
        assert!(!text.iter().any(|t| t.contains("SECRET-")), "{text:?}");
    }

    #[test]
    fn secrets_in_the_wrong_shape_stay_out_of_summaries() {
        let s = crate::secret::Secret::generate().encode();
        let old =
            json!({"clients": {"home": {"secret": "A"}}, "visitors": {"ops": {"secret": "B"}}});
        let new = json!({
            "clients": {"home": {"secret": "A", "secert": s}, "office": s},
            "visitors": {"ops": {"secret": "B", "note": format!("key {s}")}},
            "token_list": [s],
            "proxy": format!("http://u:{s}@p"),
        });
        let c = changes(&old, &new, &[]);
        let text: Vec<&str> = c.iter().map(|c| c.summary.as_str()).collect();
        assert!(!text.iter().any(|t| t.contains(&s)), "{text:?}");
        assert!(
            !changes(&new, &old, &[])
                .iter()
                .any(|c| c.summary.contains(&s))
        );
    }
}
