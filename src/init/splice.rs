use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::config::emit::render;
use crate::config::{Format, tree};

fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

pub fn allow_note(client: &str, allow: &Option<Vec<String>>) -> String {
    let note = match allow.as_deref() {
        Some([one]) if one == "any" => format!("client {client} allows any target"),
        Some([]) => format!("client {client} allows nothing yet"),
        Some(rules) => format!("client {client} allows {}", rules.join(", ")),
        None => format!("client {client} is offline; what it allows is unknown"),
    };
    clean(&note)
}

pub fn reach_note(client: &crate::protocol::GrantedClient) -> String {
    if client.reach.is_empty() {
        return allow_note(&client.name, &client.allow);
    }
    clean(&format!(
        "client {} lets you reach {}",
        client.name,
        client.reach.join(", ")
    ))
}

fn is_note(line: &str) -> bool {
    let line = line.trim();
    line.starts_with("# client ")
        && (line.contains(" allows ")
            || line.contains(" is offline")
            || line.contains(" lets you reach "))
}

fn entry_yaml(key: &str, value: &Value, note: Option<&String>, indent: &str) -> String {
    let mut out = String::new();
    if let Some(note) = note {
        out.push_str(&format!("{indent}# {note}\n"));
    }
    let mut one = Map::new();
    one.insert(key.to_string(), value.clone());
    for line in render(&Value::Object(one), Format::Yaml).lines() {
        out.push_str(indent);
        out.push_str(line);
        out.push('\n');
    }
    out
}

pub fn forwards_yaml(forwards: &Map<String, Value>, notes: &BTreeMap<String, String>) -> String {
    if forwards.is_empty() {
        return "forwards: {}\n".into();
    }
    let mut out = String::from("forwards:\n");
    for (key, value) in forwards {
        out.push_str(&entry_yaml(key, value, notes.get(key), "  "));
    }
    out
}

fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |i| i + 1)
}

fn line_end(text: &str, at: usize) -> usize {
    if at == 0 || text[..at].ends_with('\n') {
        return at;
    }
    text[at..].find('\n').map_or(text.len(), |i| at + i + 1)
}

fn own_lines(block: &str, indent: usize) -> usize {
    let mut keep = block.len();
    for line in block.split_inclusive('\n').rev() {
        let body = line.trim_start();
        let comment = body.starts_with('#') && line.len() - body.len() <= indent;
        if line.trim_end().is_empty() || comment {
            keep -= line.len();
        } else {
            break;
        }
    }
    keep
}

fn end_of(src: &str, start: usize, node_end: usize, indent: usize) -> usize {
    let end = line_end(src, node_end.max(start));
    start + own_lines(&src[start..end], indent)
}

pub struct Edit<'a> {
    pub removed: &'a [String],
    pub added: &'a Map<String, Value>,
    pub notes: &'a BTreeMap<String, String>,
}

fn previous_line(src: &str, at: usize) -> Option<(usize, &str)> {
    if at == 0 {
        return None;
    }
    let start = line_start(src, at - 1);
    Some((start, &src[start..at]))
}

fn lead(src: &str, start: usize, indent: &str) -> usize {
    let mut at = start;
    while let Some((before, line)) = previous_line(src, at) {
        match line.strip_prefix(indent) {
            Some(rest) if rest.starts_with('#') => at = before,
            _ => break,
        }
    }
    at
}

fn edit_block(
    src: &str,
    forwards: &tree::Entry,
    entries: &[tree::Entry],
    edit: &Edit,
) -> Vec<(usize, usize, String)> {
    let mut changes = Vec::new();
    let left = !edit.added.is_empty() || entries.iter().any(|e| !edit.removed.contains(&e.key));
    if !left {
        let colon = forwards.key_span.end.offset
            + src[forwards.key_span.end.offset..].find(':').unwrap_or(0)
            + 1;
        changes.push((colon, colon, " {}".to_string()));
    }
    let first = line_start(src, entries[0].key_span.start.offset);
    let indent = &src[first..entries[0].key_span.start.offset];
    let starts: Vec<usize> = entries
        .iter()
        .map(|e| line_start(src, e.key_span.start.offset))
        .collect();
    let ends: Vec<usize> = (0..entries.len())
        .map(|i| match starts.get(i + 1) {
            Some(&next) => {
                starts[i] + own_lines(&src[starts[i]..lead(src, next, indent)], indent.len())
            }
            None => end_of(
                src,
                starts[i],
                entries[i].value.span.end.offset,
                indent.len(),
            ),
        })
        .collect();
    for (i, e) in entries.iter().enumerate() {
        let start = starts[i];
        let note_above = previous_line(src, start).filter(|(_, line)| is_note(line));
        if edit.removed.contains(&e.key) {
            changes.push((lead(src, start, indent), ends[i], String::new()));
        } else if let Some(note) = edit.notes.get(&e.key) {
            let line = format!("{indent}# {note}\n");
            match note_above {
                Some((at, _)) => changes.push((at, start, line)),
                None => changes.push((start, start, line)),
            }
        }
    }
    let after = ends[entries.len() - 1];
    let added: String = edit
        .added
        .iter()
        .map(|(k, v)| entry_yaml(k, v, edit.notes.get(k), indent))
        .collect();
    if !added.is_empty() {
        let lead = if src[..after].ends_with('\n') {
            ""
        } else {
            "\n"
        };
        changes.push((after, after, format!("{lead}{added}")));
    }
    changes
}

fn comment_at(line: &str) -> Option<usize> {
    let mut quote = None;
    let mut before = ' ';
    for (i, c) in line.char_indices() {
        match (quote, c) {
            (None, '#') if before.is_whitespace() => return Some(i),
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            _ => {}
        }
        before = c;
    }
    None
}

fn trailing_comment(block: &str) -> Option<&str> {
    block
        .lines()
        .rev()
        .find(|line| {
            let content = comment_at(line).map_or(*line, |at| &line[..at]);
            !content.trim().is_empty()
        })
        .and_then(|line| comment_at(line).map(|at| line[at..].trim_end()))
}

fn rebuild(src: &str, entries: &[tree::Entry], edit: &Edit) -> String {
    let mut out = String::from("forwards:\n");
    for e in entries.iter().filter(|e| !edit.removed.contains(&e.key)) {
        if let Some(note) = edit.notes.get(&e.key) {
            out.push_str(&format!("  # {note}\n"));
        }
        let key = &src[e.key_span.start.offset..e.key_span.end.offset];
        let value = src[e.value.span.start.offset..e.value.span.end.offset].trim_end();
        out.push_str(&format!("  {key}: {value}\n"));
    }
    for (k, v) in edit.added {
        out.push_str(&entry_yaml(k, v, edit.notes.get(k), "  "));
    }
    if out == "forwards:\n" {
        out = "forwards: {}\n".into();
    }
    out
}

pub fn update_yaml(text: &str, edit: &Edit) -> Option<String> {
    let (bom, body) = match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text),
    };
    let crlf = body.contains("\r\n");
    let src = if crlf {
        body.replace("\r\n", "\n")
    } else {
        body.to_string()
    };
    let tree::Value::Map(top) = tree::parse(&src).ok()?.value else {
        return None;
    };
    let out = match top.iter().find(|e| e.key == "forwards") {
        None => {
            let mut out = src.clone();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&forwards_yaml(edit.added, edit.notes));
            out
        }
        Some(f) => {
            let tree::Value::Map(entries) = &f.value.value else {
                return None;
            };
            let block = !entries.is_empty() && !src[f.value.span.start.offset..].starts_with('{');
            if block {
                let mut out = src.clone();
                let mut changes = edit_block(&src, f, entries, edit);
                changes.sort_by_key(|c| std::cmp::Reverse((c.0, c.1)));
                for (from, to, with) in changes {
                    out.replace_range(from..to, &with);
                }
                out
            } else {
                let start = line_start(&src, f.key_span.start.offset);
                let value_end = f.value.span.end.offset.max(f.key_span.end.offset);
                let end = end_of(&src, start, value_end, 0);
                let mut rebuilt = rebuild(&src, entries, edit);
                if let Some(comment) = trailing_comment(&src[start..end]) {
                    let first = rebuilt.find('\n').unwrap_or(rebuilt.len());
                    rebuilt.insert_str(first, &format!(" {comment}"));
                }
                format!("{}{rebuilt}{}", &src[..start], &src[end..])
            }
        }
    };
    let out = if crlf { out.replace('\n', "\r\n") } else { out };
    Some(format!("{bom}{out}"))
}

pub fn update_json(text: &str, edit: &Edit) -> Option<String> {
    let (bom, body) = match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text),
    };
    let mut doc: Value = serde_json::from_str(body).ok()?;
    let forwards = doc
        .as_object_mut()?
        .entry("forwards")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()?;
    for name in edit.removed {
        forwards.remove(name);
    }
    for (k, v) in edit.added {
        forwards.insert(k.clone(), v.clone());
    }
    let out = render(&doc, Format::Json);
    let out = if body.contains("\r\n") {
        out.replace('\n', "\r\n")
    } else {
        out
    };
    Some(format!("{bom}{out}"))
}

pub fn new_json(server: &str, name: &str, secret: &str, forwards: &Map<String, Value>) -> String {
    render(
        &json!({ "server": server, "name": name, "secret": secret, "forwards": forwards }),
        Format::Json,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn added(key: &str, value: Value) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert(key.into(), value);
        m
    }

    fn check(text: &str) {
        let secret = crate::secret::Secret::generate().encode();
        let env = |k: &str| match k {
            "S" => Some(secret.clone()),
            "P" => Some("2222".to_string()),
            _ => None,
        };
        let parsed = crate::config::load_str(
            "visitor.yaml",
            text,
            Format::Yaml,
            Some(crate::config::Role::Visitor),
            &env,
        );
        assert!(
            !parsed.report.has_errors(),
            "{}\n{text}",
            parsed
                .report
                .render_text(crate::diagnostics::Style { color: false })
        );
    }

    const FILE: &str = "# my laptop\nserver: tunnel.example.com\nname: laptop\nsecret: ${S} # from the admin\nforwards:\n  # my ssh, keep it\n  ssh: { client: home, target: 22, local: \"${P}\" }\n  old: { service: old, local: 7000 }\n  # client home allows 10.0.0.0/8\n  home:\n    client: home\n    socks: true\n    local: 1080\n# after\nproxy: http://p:3128\n";

    #[test]
    fn kept_forwards_keep_their_text_and_only_changes_are_written() {
        let notes = BTreeMap::from([
            (
                "home".to_string(),
                allow_note("home", &Some(vec!["127.0.0.0/8".into()])),
            ),
            ("web".to_string(), "x".to_string()),
        ]);
        let new = added("web", json!({ "service": "web", "local": 8080 }));
        let out = update_yaml(
            FILE,
            &Edit {
                removed: &["old".to_string()],
                added: &new,
                notes: &notes,
            },
        )
        .unwrap();
        assert!(
            out.contains(
                "  # my ssh, keep it\n  ssh: { client: home, target: 22, local: \"${P}\" }\n"
            ),
            "{out}"
        );
        assert!(!out.contains("old"), "{out}");
        assert!(
            out.contains("  # client home allows 127.0.0.0/8\n  home:\n"),
            "{out}"
        );
        assert!(
            !out.contains("10.0.0.0/8"),
            "the old note is replaced: {out}"
        );
        assert!(
            out.contains("    local: 1080\n  # x\n  web:\n    service: web\n    local: 8080\n# after\nproxy: http://p:3128\n"),
            "{out}"
        );
        assert!(out.starts_with("# my laptop\n"), "{out}");
        check(&out);
    }

    #[test]
    fn removing_an_entry_keeps_the_next_one_and_its_note() {
        let file = "server: x\nname: y\nsecret: ${S}\nforwards:\n  # old one\n  db:\n    service: db\n    local: 15432\n  # client home allows 127.0.0.0/8\n  home:\n    client: home\n    socks: true\n    local: 1080\n# the end\n";
        let new = added("web", json!({ "service": "web", "local": 8080 }));
        let out = update_yaml(
            file,
            &Edit {
                removed: &["db".to_string()],
                added: &new,
                notes: &BTreeMap::new(),
            },
        )
        .unwrap();
        assert_eq!(
            out,
            "server: x\nname: y\nsecret: ${S}\nforwards:\n  # client home allows 127.0.0.0/8\n  home:\n    client: home\n    socks: true\n    local: 1080\n  web:\n    service: web\n    local: 8080\n# the end\n"
        );
        check(&out);
        let last = update_yaml(
            file,
            &Edit {
                removed: &["home".to_string()],
                added: &Map::new(),
                notes: &BTreeMap::new(),
            },
        )
        .unwrap();
        assert_eq!(
            last,
            "server: x\nname: y\nsecret: ${S}\nforwards:\n  # old one\n  db:\n    service: db\n    local: 15432\n# the end\n"
        );
        check(&last);
        let all = update_yaml(
            file,
            &Edit {
                removed: &["db".to_string(), "home".to_string()],
                added: &Map::new(),
                notes: &BTreeMap::new(),
            },
        )
        .unwrap();
        assert_eq!(
            all,
            "server: x\nname: y\nsecret: ${S}\nforwards: {}\n# the end\n"
        );
        check(&all);
    }

    #[test]
    fn comments_after_the_last_entry_and_around_the_block_stay() {
        let file = "forwards: # mine\n  a: { service: a, local: 1 }\n  b:\n    service: b\n    local: 2\n  # disabled: { service: d, local: 4 }\n# section\nproxy: http://p:1\n";
        let none = Map::new();
        let notes = BTreeMap::new();
        let edit = |removed: &[String]| {
            update_yaml(
                file,
                &Edit {
                    removed,
                    added: &none,
                    notes: &notes,
                },
            )
            .unwrap()
        };
        assert_eq!(
            edit(&["b".to_string()]),
            "forwards: # mine\n  a: { service: a, local: 1 }\n  # disabled: { service: d, local: 4 }\n# section\nproxy: http://p:1\n"
        );
        assert_eq!(
            edit(&["a".to_string(), "b".to_string()]),
            "forwards: {} # mine\n  # disabled: { service: d, local: 4 }\n# section\nproxy: http://p:1\n"
        );
        let flow = update_yaml(
            "forwards: { a: { service: a, local: 1 } } # keep me\nproxy: http://p:1\n",
            &Edit {
                removed: &["a".to_string()],
                added: &none,
                notes: &notes,
            },
        )
        .unwrap();
        assert_eq!(flow, "forwards: {} # keep me\nproxy: http://p:1\n");
    }

    #[test]
    fn crlf_and_a_bom_are_kept() {
        let text = format!("\u{feff}{}", FILE.replace('\n', "\r\n"));
        let out = update_yaml(
            &text,
            &Edit {
                removed: &["old".to_string()],
                added: &Map::new(),
                notes: &BTreeMap::new(),
            },
        )
        .unwrap();
        assert!(out.starts_with('\u{feff}'));
        assert!(
            !out.replace("\r\n", "").contains('\n'),
            "only CRLF line ends: {out:?}"
        );
        assert!(!out.contains("old"));
    }

    #[test]
    fn flow_blocks_and_missing_blocks_are_rebuilt_without_losing_values() {
        let flow = "server: x\nname: y\nsecret: ${S}\nforwards: { ssh: { client: home, target: 22, local: \"${P}\" }, old: { service: old, local: 7 } }\n";
        let new = added("web", json!({ "service": "web", "local": 8080 }));
        let edit = Edit {
            removed: &["old".to_string()],
            added: &new,
            notes: &BTreeMap::new(),
        };
        let out = update_yaml(flow, &edit).unwrap();
        assert!(
            out.contains("  ssh: { client: home, target: 22, local: \"${P}\" }\n"),
            "{out}"
        );
        assert!(out.contains("  web:\n    service: web\n"), "{out}");
        check(&out);
        let none = update_yaml("server: x\nname: y\nsecret: ${S}", &edit).unwrap();
        assert!(
            none.ends_with("secret: ${S}\nforwards:\n  web:\n    service: web\n    local: 8080\n"),
            "{none}"
        );
        let empty = update_yaml("server: x\nname: y\nsecret: ${S}\nforwards: {}\n", &edit).unwrap();
        check(&empty);
    }

    #[test]
    fn json_keeps_its_bom_and_crlf() {
        let text = "\u{feff}{\r\n  \"server\": \"x\",\r\n  \"forwards\": {}\r\n}\r\n";
        let out = update_json(
            text,
            &Edit {
                removed: &[],
                added: &added("web", json!({ "service": "web", "local": 8080 })),
                notes: &BTreeMap::new(),
            },
        )
        .unwrap();
        assert!(out.starts_with('\u{feff}'), "{out:?}");
        assert!(!out.replace("\r\n", "").contains('\n'), "{out:?}");
        let v: Value = serde_json::from_str(out.trim_start_matches('\u{feff}')).unwrap();
        assert_eq!(v["forwards"]["web"]["local"], 8080);
    }

    #[test]
    fn json_keeps_its_other_keys_and_values() {
        let out = update_json(
            r#"{"server": "x", "name": "y", "secret": "${S}", "forwards": {"ssh": {"client": "home", "target": "22", "local": "${P}"}, "old": {"service": "old", "local": 7}}}"#,
            &Edit {
                removed: &["old".to_string()],
                added: &added("web", json!({ "service": "web", "local": 8080 })),
                notes: &BTreeMap::new(),
            },
        )
        .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["secret"], "${S}");
        assert_eq!(v["forwards"]["ssh"]["local"], "${P}");
        assert!(v["forwards"].get("old").is_none());
        assert_eq!(v["forwards"]["web"]["local"], 8080);
    }

    #[test]
    fn notes_say_what_is_known_and_cannot_break_out_of_the_comment() {
        assert_eq!(
            allow_note("home", &Some(vec!["any".into()])),
            "client home allows any target"
        );
        assert_eq!(
            allow_note("home", &None),
            "client home is offline; what it allows is unknown"
        );
        let sneaky = allow_note(
            "home",
            &Some(vec!["unix:/x\n  evil:\n    client: home".into()]),
        );
        assert!(!sneaky.contains('\n'), "{sneaky}");
    }
}
