use burrow_admin_proto::codes;
use burrow_config::{ClientEntry, Entry as _, ServiceEntry, VisitorEntry};

use crate::config::diagnostics::Diagnostic;
use crate::config::tree::{Entry, Node, Span, Value};

fn sections() -> [(&'static str, &'static str); 3] {
    [
        (ServiceEntry::section(), "service"),
        (ClientEntry::section(), "client"),
        (VisitorEntry::section(), "visitor"),
    ]
}

pub fn is_section(key: &str) -> bool {
    sections().iter().any(|(s, _)| *s == key)
}

fn empty() -> Node {
    Node {
        value: Value::Null,
        span: Span::default(),
        quoted: false,
        env: Default::default(),
        raw: None,
    }
}

pub fn merge(trees: Vec<Node>) -> Node {
    merge_with(trees, &[]).0
}

pub fn merge_with(trees: Vec<Node>, names: &[String]) -> (Node, Vec<Diagnostic>) {
    let mut out = Vec::new();
    let mut trees = trees.into_iter();
    let Some(mut root) = trees.next() else {
        return (empty(), out);
    };
    let place = |s: Span| {
        format!(
            "{}:{}",
            names.get(s.file.index()).map_or("", String::as_str),
            s.start.line
        )
    };
    for more in trees {
        append(&mut root, more, &place, &mut out);
    }
    (root, out)
}

fn append(root: &mut Node, more: Node, place: &dyn Fn(Span) -> String, out: &mut Vec<Diagnostic>) {
    let Value::Map(extra) = more.value else {
        return;
    };
    if matches!(root.value, Value::Null) {
        root.value = Value::Map(Vec::new());
    }
    let Value::Map(top) = &mut root.value else {
        return;
    };
    for entry in extra {
        let Some((section, kind)) = sections().into_iter().find(|(s, _)| *s == entry.key) else {
            out.push(
                Diagnostic::error(
                    codes::DROP_IN_KEY,
                    format!("\"{}\" cannot be set in a drop-in file", entry.key),
                )
                .at(entry.key_span)
                .label("server-wide setting")
                .help("server-wide settings belong in the main config file; a drop-in file holds only services, clients and visitors"),
            );
            continue;
        };
        let added = match entry.value.value {
            Value::Map(items) => items,
            Value::Null => continue,
            _ => {
                out.push(
                    Diagnostic::error(
                        codes::DROP_IN_SECTION,
                        format!("{section} in a drop-in file must list {kind}s by name"),
                    )
                    .at(entry.value.span)
                    .help(format!("write it as\n{section}:\n  name: {{ ... }}")),
                );
                continue;
            }
        };
        let have = match top.iter_mut().find(|e| e.key == section) {
            Some(have) => have,
            None => {
                top.push(Entry {
                    key: entry.key,
                    key_span: entry.key_span,
                    value: Node {
                        value: Value::Map(Vec::new()),
                        ..entry.value
                    },
                });
                top.last_mut().expect("just pushed")
            }
        };
        if matches!(have.value.value, Value::Null) {
            have.value.value = Value::Map(Vec::new());
        }
        let Value::Map(items) = &mut have.value.value else {
            continue;
        };
        for item in added {
            if let Some(first) = items.iter().find(|i| i.key == item.key) {
                out.push(
                    Diagnostic::error(
                        codes::DUPLICATE_NAME,
                        format!(
                            "{kind} \"{}\" is defined twice: at {} and at {}",
                            item.key,
                            place(first.key_span),
                            place(item.key_span)
                        ),
                    )
                    .at(item.key_span)
                    .label("defined again here")
                    .about(item.key.clone())
                    .help(format!(
                        "keep one of them, or rename this {kind}; the same name in two files is never merged"
                    )),
                );
                continue;
            }
            items.push(item);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tree::{FileId, parse_in};

    #[test]
    fn items_of_a_second_file_join_the_sections_of_the_first_and_keep_their_file() {
        let main = parse_in(
            "listen: 7835\nservices:\n  web: { target: 80 }\n",
            FileId(0),
        )
        .unwrap();
        let more = parse_in(
            "services:\n  blog: { target: 81 }\nclients:\n  home: {}\n",
            FileId(1),
        )
        .unwrap();
        let root = merge(vec![main, more]);
        let services = root.get("services").unwrap();
        let web = services.entry("web").unwrap();
        let blog = services.entry("blog").unwrap();
        assert_eq!(web.key_span.file, FileId(0));
        assert_eq!(
            (blog.key_span.file, blog.key_span.start.line),
            (FileId(1), 2)
        );
        assert_eq!(root.entry("clients").unwrap().key_span.file, FileId(1));
        assert_eq!(
            root.to_json(),
            serde_json::json!({"listen": 7835, "services": {"web": {"target": 80}, "blog": {"target": 81}}, "clients": {"home": {}}})
        );
    }

    fn merged(texts: &[&str]) -> (Node, Vec<Diagnostic>) {
        let names: Vec<String> = ["server.yaml", "server.d/a.yaml", "server.d/b.yaml"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let trees = texts
            .iter()
            .enumerate()
            .map(|(i, t)| parse_in(t, FileId(i as u16)).unwrap())
            .collect();
        merge_with(trees, &names)
    }

    #[test]
    fn a_server_wide_setting_in_a_drop_in_is_an_error_at_its_line() {
        let (root, d) = merged(&["listen: 7835\n", "services: {}\nlisten: 9000\nweb: {}\n"]);
        assert_eq!(root.get("listen").unwrap().value, Value::Int(7835));
        let keys: Vec<_> = d.iter().filter(|d| d.code == codes::DROP_IN_KEY).collect();
        assert_eq!(keys.len(), 2);
        let s = keys[0].span.unwrap();
        assert_eq!((s.file, s.start.line), (FileId(1), 2));
    }

    #[test]
    fn the_same_name_in_two_files_names_both_places() {
        let (root, d) = merged(&[
            "services:\n  web: { target: 80 }\n",
            "clients:\n  home: {}\n",
            "\nclients:\n  home: { x: 1 }\nservices:\n  web: { target: 81 }\n",
        ]);
        let dup: Vec<_> = d
            .iter()
            .filter(|d| d.code == codes::DUPLICATE_NAME)
            .collect();
        assert_eq!(dup.len(), 2, "{d:?}");
        assert_eq!(
            dup[0].message,
            "client \"home\" is defined twice: at server.d/a.yaml:2 and at server.d/b.yaml:3"
        );
        assert_eq!(
            dup[1].message,
            "service \"web\" is defined twice: at server.yaml:2 and at server.d/b.yaml:5"
        );
        assert_eq!(
            root.get("services").unwrap().get("web").unwrap().to_json(),
            serde_json::json!({"target": 80})
        );
    }

    #[test]
    fn a_section_that_is_not_a_list_of_names_is_an_error() {
        let (_, d) = merged(&["listen: 7835\n", "services: [web]\nclients:\n"]);
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].code, codes::DROP_IN_SECTION);
    }
}
