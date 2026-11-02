use std::collections::BTreeMap;

use burrow_admin_proto::plan::{FileChange, FileDiff};
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

pub type Places = BTreeMap<Item, usize>;

pub fn split(
    owners: &Owners,
    (count, places): (usize, &Places),
    target: &Value,
    renames: &[Rename],
) -> Vec<Value> {
    let mut outs: Vec<Map<String, Value>> = vec![Map::new(); count];
    let Value::Object(top) = target else {
        let mut all = vec![Value::Object(Map::new()); count];
        all[0] = target.clone();
        return all;
    };
    for (key, value) in top {
        let owner_of_key = || owners.keys.get(key).copied().unwrap_or(0).min(count - 1);
        let (true, Value::Object(items)) = (sections().contains(&key.as_str()), value) else {
            outs[owner_of_key()].insert(key.clone(), value.clone());
            continue;
        };
        let mut parts: Vec<Map<String, Value>> = vec![Map::new(); count];
        for (name, item) in items {
            let owner = places
                .get(&(key.clone(), name.clone()))
                .copied()
                .unwrap_or_else(|| owners.file_of(key, &was_named(renames, key, name)))
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

fn renames_in(owners: &Owners, places: &Places, doc: usize, renames: &[Rename]) -> Vec<Rename> {
    renames
        .iter()
        .filter(|r| match r.path.as_slice() {
            [section, name, ..] if sections().contains(&section.as_str()) => {
                let moved = places
                    .get(&(section.clone(), r.to.clone()))
                    .is_some_and(|to| *to != doc);
                owners.file_of(section, name) == doc && !moved
            }
            _ => doc == 0,
        })
        .cloned()
        .collect()
}

pub fn holds_items(target: &Value) -> bool {
    sections().iter().any(|s| {
        target
            .get(*s)
            .and_then(Value::as_object)
            .is_some_and(|m| !m.is_empty())
    })
}

pub fn valid_name(prefix: &str, name: &str) -> bool {
    let Some(file) = name
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('/'))
    else {
        return false;
    };
    let stem = file.strip_suffix(".yaml").unwrap_or("");
    let device = stem.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&device.as_str())
        || ((device.starts_with("COM") || device.starts_with("LPT"))
            && device.len() == 4
            && device.ends_with(|c: char| c.is_ascii_digit()));
    !stem.is_empty()
        && stem.len() <= 100
        && !reserved
        && !stem.contains("..")
        && !stem.starts_with(['.', '-'])
        && !stem.ends_with('.')
        && stem
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

pub struct Written {
    pub docs: Docs,
    pub reformat: Option<patch::Reformat>,
}

#[derive(Debug)]
pub enum WriteError {
    Patch(patch::PatchError),
    Bad(String),
}

pub struct Moves<'a> {
    pub places: &'a [(Vec<String>, String)],
    pub drops: &'a [String],
    pub prefix: &'a str,
}

impl Moves<'_> {
    pub const NONE: Moves<'static> = Moves {
        places: &[],
        drops: &[],
        prefix: "",
    };
}

fn placed(base: &mut Vec<Doc>, moves: &Moves) -> Result<Places, WriteError> {
    let mut out = Places::new();
    for (path, file) in moves.places {
        let [section, name] = path.as_slice() else {
            return Err(WriteError::Bad(
                "place needs a service, client or visitor".into(),
            ));
        };
        if !sections().contains(&section.as_str()) {
            return Err(WriteError::Bad(format!(
                "{section} items stay in the main file"
            )));
        }
        let at = match base.iter().position(|d| d.name == *file) {
            Some(at) => at,
            None if valid_name(moves.prefix, file) => {
                base.push(Doc {
                    name: file.clone(),
                    text: String::new(),
                });
                base.len() - 1
            }
            None => {
                return Err(WriteError::Bad(format!(
                    "\"{file}\" is not a file name burrow can use; use {}/<name>.yaml with letters, digits, dots, dashes or underscores",
                    moves.prefix
                )));
            }
        };
        out.insert((section.clone(), name.clone()), at);
    }
    Ok(out)
}

pub fn write(
    base: &Docs,
    main_format: Format,
    (target, renames): (&Value, &[Rename]),
    moves: &Moves,
) -> Result<Written, WriteError> {
    let owners = Owners::of(base);
    let mut docs = base.list.clone();
    let places = placed(&mut docs, moves)?;
    let targets = split(&owners, (docs.len(), &places), target, renames);
    let mut list = Vec::with_capacity(docs.len());
    let mut reformat = None;
    for (i, (doc, target)) in docs.iter().zip(&targets).enumerate() {
        if moves.drops.contains(&doc.name) {
            if i == 0 || holds_items(target) {
                return Err(WriteError::Bad(format!(
                    "{} cannot be deleted: it still holds items",
                    doc.name
                )));
            }
            continue;
        }
        if i >= base.list.len() {
            list.push(Doc {
                name: doc.name.clone(),
                text: crate::config::emit::render(target, Format::Yaml),
            });
            continue;
        }
        if i > 0 && owners.raws.get(i) == Some(target) {
            list.push(doc.clone());
            continue;
        }
        let format = if i == 0 { main_format } else { Format::Yaml };
        let Patched { text, reformat: r } = patch::patch(
            &doc.text,
            format,
            target,
            &renames_in(&owners, &places, i, renames),
        )
        .map_err(WriteError::Patch)?;
        reformat = reformat.or(r);
        list.push(Doc {
            name: doc.name.clone(),
            text,
        });
    }
    if let Some(gone) = moves.drops.iter().find(|d| base.get(d).is_none()) {
        return Err(WriteError::Bad(format!(
            "{gone} is not one of the config files"
        )));
    }
    Ok(Written {
        docs: Docs { list },
        reformat,
    })
}

pub fn diffs(old: &Docs, new: &Docs, mask: &dyn Fn(&str) -> String) -> Vec<FileDiff> {
    if old.list.len() < 2 && new.list.len() < 2 {
        return Vec::new();
    }
    let mut names: Vec<&str> = old.names().collect();
    names.extend(new.names().filter(|n| old.get(n).is_none()));
    names
        .into_iter()
        .map(|name| {
            let (a, b) = (old.text_of(name), new.text_of(name));
            let change = match (a, b) {
                (Some(a), Some(b)) if a == b => FileChange::Same,
                (Some(_), Some(_)) => FileChange::Edit,
                (None, _) => FileChange::Create,
                (Some(_), None) => FileChange::Remove,
            };
            let left_empty = (name != new.main().name)
                .then(|| b.map(empty_of_items))
                .flatten()
                .filter(|e| *e && change != FileChange::Same);
            FileDiff {
                file: name.to_string(),
                diff: if change == FileChange::Same {
                    Default::default()
                } else {
                    crate::admin::core::diff::unified(a.unwrap_or(""), b.unwrap_or(""), mask)
                },
                change,
                left_empty,
            }
        })
        .collect()
}

fn empty_of_items(text: &str) -> bool {
    let (_, _, src) = patch::split_text(text);
    tree::parse(&src).is_ok_and(|t| !holds_items(&t.to_json()))
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
        let w = write(&base, Format::Yaml, (&target, &renames), &Moves::NONE).unwrap();
        assert_eq!(w.docs.list[0].text, base.list[0].text);
        assert_eq!(
            w.docs.list[1].text,
            "# the blog\nservices:\n  journal:\n    client: home # main box\n    target: 82\n"
        );
    }

    #[test]
    fn only_a_plain_name_inside_the_folder_makes_a_new_file() {
        for good in [
            "server.d/blog.yaml",
            "server.d/team-a_1.yaml",
            "server.d/v1.2.yaml",
        ] {
            assert!(valid_name("server.d", good), "{good}");
        }
        for bad in [
            "server.d/../x.yaml",
            "server.d/a/b.yaml",
            "server.d\\b.yaml",
            "server.d/.hidden.yaml",
            "server.d/-x.yaml",
            "server.d/x..yaml",
            "server.d/x.yml",
            "server.d/.yaml",
            "server.d/CON.yaml",
            "server.d/nul.yaml",
            "server.d/com1.yaml",
            "server.d/lpt9.x.yaml",
            "server.d/x\0.yaml",
            "server.d/x .yaml",
            "server.d/é.yaml",
            "other.d/x.yaml",
            "server.yaml",
            "/etc/x.yaml",
        ] {
            assert!(!valid_name("server.d", bad), "{bad}");
        }
        let base = docs(&[("server.yaml", "listen: 1\n")]);
        let places = [(
            vec!["services".to_string(), "x".to_string()],
            "server.d/../x.yaml".to_string(),
        )];
        let moves = Moves {
            places: &places,
            drops: &[],
            prefix: "server.d",
        };
        let target = json!({"listen": 1, "services": {"x": {}}});
        assert!(matches!(
            write(&base, Format::Yaml, (&target, &[]), &moves),
            Err(WriteError::Bad(_))
        ));
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
        let w = write(&base, Format::Yaml, (&target, &[]), &Moves::NONE).unwrap();
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
