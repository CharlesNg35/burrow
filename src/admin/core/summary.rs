use burrow_admin_proto::plan::{Change, ChangeKind};
use burrow_config::{ClientEntry, Entry, ServiceEntry, VisitorEntry};
use serde_json::Value;

use crate::config::check::secretish;
use crate::config::patch::Rename;

fn and_list(items: Vec<String>) -> String {
    match items.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => items.concat(),
    }
}

fn item(v: &Value) -> String {
    match v {
        Value::Object(m) if m.get("name").is_some_and(Value::is_string) => {
            let name = m.get("name").map(short).unwrap_or_default();
            let rest: Vec<String> = m
                .iter()
                .filter(|(k, _)| k.as_str() != "name")
                .map(|(_, x)| short(x))
                .collect();
            if rest.is_empty() {
                name
            } else {
                format!("{name} ({})", rest.join(", "))
            }
        }
        other => short(other),
    }
}

fn short(v: &Value) -> String {
    let text = match v {
        Value::Null => "nothing".to_string(),
        Value::String(s) => s.clone(),
        Value::Array(a) if a.is_empty() => "none".into(),
        Value::Array(a) => and_list(a.iter().map(item).collect()),
        Value::Object(m) if m.is_empty() => "nothing".into(),
        Value::Object(m) => m
            .iter()
            .map(|(k, x)| format!("{k} {}", short(x)))
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

fn named(path: &[String]) -> String {
    burrow_config::label_of(path).unwrap_or_else(|| path.last().cloned().unwrap_or_default())
}

fn under(path: &[String], key: &str) -> Vec<String> {
    let mut p = path.to_vec();
    p.push(key.to_string());
    p
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
            None => parts.push(format!("{} set to {}", named(&under(&path, k)), short(nv))),
            Some(Value::Object(om)) if nv.is_object() => {
                let nm = nv.as_object().unwrap_or(&empty);
                let mut what: Vec<String> = Vec::new();
                for (sub, x) in nm {
                    let at = named(&under(&under(&path, k), sub));
                    match om.get(sub) {
                        None => what.push(format!("{at} added ({})", short(x))),
                        Some(y) if y != x => what.push(format!("{at} {} → {}", short(y), short(x))),
                        _ => {}
                    }
                }
                what.extend(
                    om.keys()
                        .filter(|sub| !nm.contains_key(*sub))
                        .map(|sub| format!("{} removed", named(&under(&under(&path, k), sub)))),
                );
                parts.push(format!("{}: {}", named(&under(&path, k)), and_list(what)));
            }
            Some(ov) => parts.push(format!(
                "{} {} → {}",
                named(&under(&path, k)),
                short(ov),
                short(nv)
            )),
        }
    }
    for k in o.keys().filter(|k| !n.contains_key(*k)) {
        parts.push(
            if k == ServiceEntry::WEB.key() && kind == ServiceEntry::ENTITY {
                "website turned off".to_string()
            } else {
                format!("{} removed", named(&under(&path, k)))
            },
        );
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
        } else if let Some((name, parent)) = r.path.split_last()
            && parent == burrow_config::logins_path().as_slice()
        {
            out.push(Change {
                kind: ChangeKind::Rename,
                path: r.path.clone(),
                summary: format!("Login service {name} renamed to {}", r.to),
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
        let section = named(&path);
        let field = |k: &str| named(&under(&path, k));
        let summary = match (ov, nv) {
            (Some(Value::Object(a)), Some(Value::Object(b))) => {
                let mut parts = Vec::new();
                for (k, v) in b {
                    match a.get(k) {
                        None => parts.push(format!("{} set to {}", field(k), short(v))),
                        Some(was) if was != v => {
                            parts.push(format!("{} {} → {}", field(k), short(was), short(v)))
                        }
                        _ => {}
                    }
                }
                for k in a.keys().filter(|k| !b.contains_key(*k)) {
                    parts.push(format!("{} back to its default", field(k)));
                }
                format!("{section}: {}", parts.join("; "))
            }
            (None, Some(Value::Object(b))) if !b.is_empty() => format!(
                "{section}: {}",
                b.iter()
                    .map(|(k, v)| format!("{} set to {}", field(k), short(v)))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
            (None, Some(v)) => format!("{section} set to {}", short(v)),
            (Some(Value::Object(_)), None) => format!("{section} back to their defaults"),
            (Some(_), None) => format!("{section} removed"),
            (Some(a), Some(b)) => format!("{section} {} → {}", short(a), short(b)),
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
        assert!(text.contains(&"Listen address 7835 → 7836"), "{text:?}");
        assert!(
            text.contains(&"Client home: secret added (2 accepted)"),
            "{text:?}"
        );
        assert!(text.contains(&"New client office (1 secret)"), "{text:?}");
        assert!(
            text.contains(&"Service web: Public address 7840 → 7841"),
            "{text:?}"
        );
        assert!(text.contains(&"Limits back to their defaults"), "{text:?}");
        let old = json!({"visitors": {"lap": {"reach": {"home": "any", "lab": "any"}}}});
        let new = json!({"visitors": {"lap": {"reach": {"home": ["10.0.0.0/8"], "office": "any"}}}, "self": {"allow": ["10.0.0.0/8"]}});
        let c = changes(&old, &new, &[]);
        let text: Vec<&str> = c.iter().map(|c| c.summary.as_str()).collect();
        assert!(
            text.contains(
                &"Visitor lap: Direct access to clients: home any → 10.0.0.0/8, office added (any) and lab removed"
            ),
            "{text:?}"
        );
        assert!(
            text.contains(&"This server: May reach on this server set to 10.0.0.0/8"),
            "{text:?}"
        );
        let old = json!({"services": {"pool": {"clients": ["home"], "target": 8080}}});
        let new = json!({"services": {"pool": {"clients": ["home", {"name": "office", "target": "10.0.0.13:8080"}, "lab"], "target": 8080}}});
        let text: Vec<String> = changes(&old, &new, &[])
            .into_iter()
            .map(|c| c.summary)
            .collect();
        assert_eq!(
            text,
            ["Service pool: Clients home → home, office (10.0.0.13:8080) and lab"],
            "lists read as words, never in brackets"
        );
        assert!(!text.iter().any(|t| t.contains("settings")), "{text:?}");
        assert!(!text.iter().any(|t| t.contains("SECRET-")), "{text:?}");
    }

    #[test]
    fn a_renamed_login_service_reads_as_a_rename() {
        let old = json!({"web": {"forward_auth": {"sso": {"service": "a", "path": "/a"}}}});
        let new = json!({"web": {"forward_auth": {"authelia": {"service": "a", "path": "/a"}}}});
        let r = Rename {
            path: burrow_config::logins_path()
                .into_iter()
                .chain(["sso".to_string()])
                .collect(),
            to: "authelia".into(),
        };
        let c = changes(&old, &new, &[r]);
        assert_eq!(c[0].kind, ChangeKind::Rename);
        assert_eq!(c[0].summary, "Login service sso renamed to authelia");
    }

    #[test]
    fn secrets_in_the_wrong_shape_stay_out_of_summaries() {
        let s = crate::wire::secret::Secret::generate().encode();
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
