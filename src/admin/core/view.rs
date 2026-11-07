use std::collections::HashMap;

use burrow_admin_proto::schema::{Node, SemType};
use burrow_admin_proto::value::{self, SecretSrc};
use burrow_admin_proto::view::{ConfigView, Shown, ViewSection};
use burrow_config::drop_ins::{self, is_section};
use serde_json::{Map, Value, json};

use super::{ApiError, Core, plan, text, values};
use crate::config::Format;
use crate::config::docs::Docs;
use crate::config::emit::render;

pub fn shown_values(core: &Core, raw: &Value, show: Shown) -> Value {
    let presented = values::present(raw, Some(&core.schema.root), "", &*core.env);
    match show {
        Shown::Written => presented,
        Shown::WithDefaults => with_defaults(&presented, &core.schema.root),
    }
}

pub fn config_view(core: &Core, show: Shown) -> Result<ConfigView, ApiError> {
    let docs = core.docs()?;
    let shown = plan::check_docs(core, &docs)
        .raw
        .map(|raw| shown_values(core, &raw, show))
        .unwrap_or(Value::Null);
    Ok(ConfigView {
        rev: text::docs_rev(&docs),
        path: core.store.path().display().to_string(),
        show,
        sections: sections(&shown, &docs),
    })
}

pub fn sections(shown: &Value, docs: &Docs) -> Vec<ViewSection> {
    let Value::Object(top) = plain(shown) else {
        return Vec::new();
    };
    let owners = owners(docs);
    let mut files = vec![Map::new(); docs.list.len().max(1)];
    for (key, v) in top {
        match v {
            Value::Object(items) if is_section(&key) && !items.is_empty() => {
                for (item, x) in items {
                    let i = owners
                        .get(&(key.clone(), item.clone()))
                        .copied()
                        .unwrap_or(0);
                    if let Value::Object(m) = files[i]
                        .entry(key.clone())
                        .or_insert_with(|| Value::Object(Map::new()))
                    {
                        m.insert(item, x);
                    }
                }
            }
            v => {
                files[0].insert(key, v);
            }
        }
    }
    files
        .into_iter()
        .enumerate()
        .flat_map(|(i, m)| {
            let origin = (i > 0).then(|| docs.list[i].name.clone());
            m.into_iter().map(move |(key, v)| ViewSection {
                text: render(&json!({ key.clone(): v }), Format::Yaml),
                key,
                origin: origin.clone(),
            })
        })
        .collect()
}

fn owners(docs: &Docs) -> HashMap<(String, String), usize> {
    let mut out = HashMap::new();
    for (i, d) in docs.list.iter().enumerate().skip(1) {
        let body = d.text.strip_prefix('\u{feff}').unwrap_or(&d.text);
        let Ok(tree) = crate::config::tree::parse(body) else {
            continue;
        };
        let json = tree.to_json();
        for (section, _) in drop_ins::sections() {
            if let Some(Value::Object(items)) = json.get(section) {
                for name in items.keys() {
                    out.insert((section.to_string(), name.clone()), i);
                }
            }
        }
    }
    out
}

pub fn plain(v: &Value) -> Value {
    if let Some(s) = value::as_secret(v) {
        return Value::String(match s.src {
            SecretSrc::Env(name) => format!("${{{name}}}"),
            SecretSrc::Literal => text::secret_shown(s.fp.as_deref()),
        });
    }
    if value::is_hash(v) {
        return Value::String(values::HASH_SHOWN.into());
    }
    if let Some(e) = value::as_env(v) {
        return Value::String(e.raw);
    }
    match v {
        Value::Object(m) => Value::Object(m.iter().map(|(k, x)| (k.clone(), plain(x))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(plain).collect()),
        other => other.clone(),
    }
}

pub fn with_defaults(v: &Value, node: &Node) -> Value {
    if value::is_special(v) {
        return v.clone();
    }
    match (&node.sem, v) {
        (SemType::Optional(o), _) => with_defaults(v, &o.inner),
        (SemType::Object(o), Value::Object(m)) => {
            let mut out = Map::new();
            for (k, x) in m {
                let field = o.fields.iter().find(|f| f.key == *k);
                out.insert(
                    k.clone(),
                    field.map_or(x.clone(), |f| with_defaults(x, &f.node)),
                );
            }
            for f in &o.fields {
                if !out.contains_key(&f.key)
                    && let Some(d) = when_absent(&f.node)
                {
                    out.insert(f.key.clone(), d);
                }
            }
            Value::Object(out)
        }
        (SemType::Map(m), Value::Object(entries)) => each_entry(entries, &m.value),
        (SemType::RefMap(m), Value::Object(entries)) => each_entry(entries, &m.value),
        (SemType::List(l), Value::Array(items)) => {
            Value::Array(items.iter().map(|i| with_defaults(i, &l.item)).collect())
        }
        _ => v.clone(),
    }
}

fn each_entry(entries: &Map<String, Value>, node: &Node) -> Value {
    Value::Object(
        entries
            .iter()
            .map(|(k, x)| (k.clone(), with_defaults(x, node)))
            .collect(),
    )
}

fn when_absent(node: &Node) -> Option<Value> {
    if let Some(d) = node.default.as_ref().filter(|d| !d.is_null()) {
        return Some(d.clone());
    }
    match &node.sem {
        SemType::Object(_) => {
            let filled = with_defaults(&Value::Object(Map::new()), node);
            filled
                .as_object()
                .is_some_and(|m| !m.is_empty())
                .then_some(filled)
        }
        _ => None,
    }
}
