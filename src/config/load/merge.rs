use crate::config::tree::{Node, Value};

pub fn merge(trees: Vec<Node>) -> Node {
    let mut trees = trees.into_iter();
    let Some(mut root) = trees.next() else {
        return Node {
            value: Value::Null,
            span: Default::default(),
            quoted: false,
            env: Default::default(),
            raw: None,
        };
    };
    for more in trees {
        append(&mut root, more);
    }
    root
}

fn append(root: &mut Node, more: Node) {
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
        match top.iter_mut().find(|e| e.key == entry.key) {
            Some(have) => match (&mut have.value.value, entry.value.value) {
                (Value::Map(items), Value::Map(added)) => items.extend(added),
                (slot @ Value::Null, added) => *slot = added,
                _ => {}
            },
            None => top.push(entry),
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
}
