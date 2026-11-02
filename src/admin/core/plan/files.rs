use std::collections::BTreeMap;

use burrow_config::{ClientEntry, Entry, ServiceEntry, VisitorEntry};
use serde_json::{Map, Value};

use crate::config::docs::{Doc, Docs};
use crate::config::patch::{self, Patched, Rename};
use crate::config::{Format, tree};

pub fn sections() -> [&'static str; 3] {
    [
        ServiceEntry::section(),
        ClientEntry::section(),
        VisitorEntry::section(),
    ]
}

pub type Item = (String, String);

#[derive(Debug, Default)]
pub struct Owners {
    pub items: BTreeMap<Item, usize>,
    keys: BTreeMap<String, usize>,
    raws: Vec<Value>,
}

impl Owners {
    pub fn of(docs: &Docs) -> Owners {
        let mut out = Owners::default();
        for (i, d) in docs.list.iter().enumerate() {
            let (_, _, src) = patch::split_text(&d.text);
            let raw = tree::parse(&src)
                .map(|t| t.to_json())
                .unwrap_or(Value::Null);
            if let Value::Object(top) = &raw {
                for (key, value) in top {
                    out.keys.entry(key.clone()).or_insert(i);
                    if let (true, Value::Object(items)) =
                        (sections().contains(&key.as_str()), value)
                    {
                        for name in items.keys() {
                            out.items.entry((key.clone(), name.clone())).or_insert(i);
                        }
                    }
                }
            }
            out.raws.push(raw);
        }
        out
    }

    pub fn file_of(&self, section: &str, name: &str) -> usize {
        self.items
            .get(&(section.to_string(), name.to_string()))
            .copied()
            .unwrap_or(0)
    }

    fn has_key(&self, doc: usize, key: &str) -> bool {
        self.raws.get(doc).and_then(|r| r.get(key)).is_some()
    }
}

fn was_named(renames: &[Rename], section: &str, name: &str) -> String {
    renames
        .iter()
        .find(|r| r.path.len() == 2 && r.path[0] == section && r.to == name)
        .map_or_else(|| name.to_string(), |r| r.path[1].clone())
}

pub fn split(owners: &Owners, count: usize, target: &Value, renames: &[Rename]) -> Vec<Value> {
    let mut outs: Vec<Map<String, Value>> = vec![Map::new(); count];
    let Value::Object(top) = target else {
        let mut all = vec![Value::Object(Map::new()); count];
        all[0] = target.clone();
        return all;
    };
    for (key, value) in top {
        let section = sections().contains(&key.as_str());
        let Value::Object(items) = value else {
            let owner = owners.keys.get(key).copied().unwrap_or(0).min(count - 1);
            outs[owner].insert(key.clone(), value.clone());
            continue;
        };
        if !section {
            let owner = owners.keys.get(key).copied().unwrap_or(0).min(count - 1);
            outs[owner].insert(key.clone(), value.clone());
            continue;
        }
        let mut parts: Vec<Map<String, Value>> = vec![Map::new(); count];
        for (name, item) in items {
            let owner = owners
                .file_of(key, &was_named(renames, key, name))
                .min(count - 1);
            parts[owner].insert(name.clone(), item.clone());
        }
        let elsewhere = (1..count).any(|i| owners.has_key(i, key));
        for (i, part) in parts.into_iter().enumerate() {
            let keep = if i == 0 {
                !part.is_empty() || owners.has_key(0, key) || !elsewhere
            } else {
                !part.is_empty() || owners.has_key(i, key)
            };
            if keep {
                outs[i].insert(key.clone(), Value::Object(part));
            }
        }
    }
    outs.into_iter().map(Value::Object).collect()
}

fn renames_in(owners: &Owners, doc: usize, renames: &[Rename]) -> Vec<Rename> {
    renames
        .iter()
        .filter(|r| match r.path.as_slice() {
            [section, name, ..] if sections().contains(&section.as_str()) => {
                owners.file_of(section, name) == doc
            }
            _ => doc == 0,
        })
        .cloned()
        .collect()
}

pub struct Written {
    pub docs: Docs,
    pub reformat: Option<patch::Reformat>,
}

pub fn write(
    base: &Docs,
    main_format: Format,
    target: &Value,
    renames: &[Rename],
) -> Result<Written, patch::PatchError> {
    let owners = Owners::of(base);
    let targets = split(&owners, base.list.len(), target, renames);
    let mut list = Vec::with_capacity(base.list.len());
    let mut reformat = None;
    for (i, (doc, target)) in base.list.iter().zip(&targets).enumerate() {
        if i > 0 && owners.raws.get(i) == Some(target) {
            list.push(doc.clone());
            continue;
        }
        let format = if i == 0 { main_format } else { Format::Yaml };
        let Patched { text, reformat: r } =
            patch::patch(&doc.text, format, target, &renames_in(&owners, i, renames))?;
        reformat = reformat.or(r);
        list.push(Doc {
            name: doc.name.clone(),
            text,
        });
    }
    Ok(Written {
        docs: Docs { list },
        reformat,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn docs(list: &[(&str, &str)]) -> Docs {
        Docs {
            list: list
                .iter()
                .map(|(n, t)| Doc {
                    name: n.to_string(),
                    text: t.to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn each_item_is_written_back_to_the_file_it_came_from() {
        let base = docs(&[
            (
                "server.yaml",
                "listen: 7835\nservices:\n  web: { client: home, target: 80 }\n",
            ),
            (
                "server.d/blog.yaml",
                "# the blog\nservices:\n  blog:\n    client: home # main box\n    target: 81\n",
            ),
        ]);
        let target = json!({
            "listen": 7835,
            "services": {
                "web": {"client": "home", "target": 80},
                "journal": {"client": "home", "target": 82}
            }
        });
        let renames = [Rename {
            path: vec!["services".into(), "blog".into()],
            to: "journal".into(),
        }];
        let w = write(&base, Format::Yaml, &target, &renames).unwrap();
        assert_eq!(w.docs.list[0].text, base.list[0].text);
        assert_eq!(
            w.docs.list[1].text,
            "# the blog\nservices:\n  journal:\n    client: home # main box\n    target: 82\n"
        );
    }

    #[test]
    fn a_new_item_goes_to_the_main_file_and_untouched_files_stay_as_they_are() {
        let base = docs(&[
            ("server.yaml", "listen: 7835\n"),
            (
                "server.d/blog.yaml",
                "services:\n  blog: { client: home, target: 81 }  # x\n",
            ),
        ]);
        let target = json!({
            "listen": 7835,
            "services": {"blog": {"client": "home", "target": 81}, "new": {"client": "home", "target": 1}}
        });
        let w = write(&base, Format::Yaml, &target, &[]).unwrap();
        assert!(
            w.docs.list[0].text.contains("new:"),
            "{}",
            w.docs.list[0].text
        );
        assert!(
            !w.docs.list[0].text.contains("blog"),
            "{}",
            w.docs.list[0].text
        );
        assert_eq!(w.docs.list[1], base.list[1]);
    }
}
