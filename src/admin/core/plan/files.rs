use std::collections::BTreeMap;

use burrow_admin_proto::plan::{Change, ChangeKind, Diff, FileChange, FileDiff};
use burrow_config::renames::{Origin, base_name, origins, within};
use burrow_config::{Entry, ServiceEntry};
use serde_json::{Map, Value};

use crate::config::docs::{Doc, Docs};
use crate::config::patch::{self, Patched, Rename};
use crate::config::{Format, tree};

fn is_section(key: &str) -> bool {
    burrow_config::drop_ins::is_section(key)
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
                for (key, value) in top.iter().filter(|(key, _)| is_section(key)) {
                    out.keys.entry(key.clone()).or_insert(i);
                    if let Value::Object(items) = value {
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

type Names = BTreeMap<String, Origin>;

fn names_in(owners: &Owners, renames: &[Rename], section: &str) -> Names {
    let base = owners
        .items
        .keys()
        .filter(|(s, _)| s == section)
        .map(|(_, name)| name.as_str());
    let renames = renames.iter().map(|r| (r.path.as_slice(), r.to.as_str()));
    origins(base, within(section, renames))
}

fn last_name(renames: &[Rename], section: &str, name: &str) -> String {
    renames
        .iter()
        .fold(name.to_string(), |name, r| match r.path.as_slice() {
            [s, from] if s == section && *from == name => r.to.clone(),
            _ => name,
        })
}

pub type Places = BTreeMap<Item, usize>;

fn home(owners: &Owners, places: &Places, names: &Names, (section, name): (&str, &str)) -> usize {
    places
        .get(&(section.to_string(), name.to_string()))
        .copied()
        .unwrap_or_else(|| base_name(names, name).map_or(0, |b| owners.file_of(section, &b)))
}

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
        let (true, Value::Object(items)) = (is_section(key), value) else {
            outs[owner_of_key()].insert(key.clone(), value.clone());
            continue;
        };
        let mut parts: Vec<Map<String, Value>> = vec![Map::new(); count];
        let names = names_in(owners, renames, key);
        for (name, item) in items {
            let owner = home(owners, places, &names, (key, name)).min(count - 1);
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
        .enumerate()
        .filter(|(k, r)| match r.path.as_slice() {
            [section, name, ..] if is_section(section) => {
                let last = last_name(&renames[*k..], section, name);
                let names = names_in(owners, renames, section);
                home(owners, places, &names, (section, &last)) == doc
            }
            _ => doc == 0,
        })
        .map(|(_, r)| r.clone())
        .collect()
}

pub fn holds_items(target: &Value) -> bool {
    burrow_config::drop_ins::sections().iter().any(|(s, _)| {
        target
            .get(*s)
            .and_then(Value::as_object)
            .is_some_and(|m| !m.is_empty())
    })
}

fn same_items(raw: &Value, target: &Value) -> bool {
    let top = |v: &Value| match v {
        Value::Null => Some(Map::new()),
        Value::Object(m) => Some(m.clone()),
        _ => None,
    };
    let (Some(mut a), Some(mut b)) = (top(raw), top(target)) else {
        return raw == target;
    };
    for m in [&mut a, &mut b] {
        for (key, value) in m.iter_mut() {
            if is_section(key) && value.is_null() {
                *value = Value::Object(Map::new());
            }
        }
    }
    a == b
}

fn read_alike(raw: &Value, read: &Value) -> bool {
    let items = |v: &Value| match v {
        Value::Null => Some(Map::new()),
        Value::Object(m) => Some(
            m.iter()
                .filter(|(key, value)| {
                    !is_section(key)
                        || value
                            .as_object()
                            .map_or(!value.is_null(), |m| !m.is_empty())
                })
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        ),
        _ => None,
    };
    match (items(raw), items(read)) {
        (Some(a), Some(b)) => a == b,
        _ => raw == read,
    }
}

fn ignored(at: usize, share: &Value, owners: &Owners, docs: &[Doc]) -> String {
    let name_of = |i: usize| docs.get(i).map_or("", |d| d.name.as_str());
    let Some(Value::Object(top)) = owners.raws.get(at) else {
        return "it is not a list of services, clients or visitors".into();
    };
    let mut out = Vec::new();
    for (key, value) in top {
        match value {
            _ if !is_section(key) => out.push(format!(
                "{key} is a server-wide setting, which only {} holds",
                name_of(0)
            )),
            Value::Null => {}
            Value::Object(items) => {
                let unread = items
                    .keys()
                    .filter(|n| share.get(key).and_then(|s| s.get(n.as_str())).is_none());
                for name in unread {
                    out.push(match owners.file_of(key, name) {
                        i if i == at => format!("{key}.{name} is not read"),
                        i => format!("{key}.{name} is also in {}", name_of(i)),
                    });
                }
            }
            _ => out.push(format!("{key} does not list items by name")),
        }
    }
    if out.is_empty() {
        return "the server reads it differently".into();
    }
    out.join(", ")
}

fn appended(text: &str, target: &Value) -> Option<String> {
    let (bom, eol, src) = patch::split_text(text);
    if !tree::parse(&src).ok()?.to_json().is_null() {
        return None;
    }
    let gap = if src.is_empty() || src.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let out = format!(
        "{src}{gap}{}",
        crate::config::emit::render(target, Format::Yaml)
    );
    let wanted = tree::parse(&out).is_ok_and(|t| t.to_json() == *target);
    wanted.then(|| format!("{bom}{}", out.replace('\n', eol)))
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

pub fn new_file(base: &[Doc], prefix: &str, file: &str) -> Result<(), WriteError> {
    let names: Vec<&str> = base.iter().map(|d| d.name.as_str()).collect();
    match burrow_config::drop_ins::new_name_problem(&names, prefix, file) {
        Some(why) => Err(WriteError::Bad(why)),
        None => Ok(()),
    }
}

fn placed(base: &mut Vec<Doc>, moves: &Moves) -> Result<Places, WriteError> {
    let mut out = Places::new();
    for (path, file) in moves.places {
        let [section, name] = path.as_slice() else {
            return Err(WriteError::Bad(
                "place needs a service, client or visitor".into(),
            ));
        };
        if !is_section(section) {
            return Err(WriteError::Bad(format!(
                "{section} items stay in the main file"
            )));
        }
        let at = match base.iter().position(|d| d.name == *file) {
            Some(at) => at,
            None => {
                new_file(base, moves.prefix, file)?;
                base.push(Doc {
                    name: file.clone(),
                    text: String::new(),
                });
                base.len() - 1
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
    let olds = match base.tree() {
        Ok(t) => split(&owners, (docs.len(), &Places::new()), &t.to_json(), &[]),
        Err(_) => owners.raws.clone(),
    };
    let mut list = Vec::with_capacity(docs.len());
    let mut reformat = None;
    for (i, (doc, target)) in docs.iter().zip(&targets).enumerate() {
        if moves.drops.contains(&doc.name) {
            if i == 0 || holds_items(target) {
                return Err(still_holds(&doc.name));
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
        if same_items(&olds[i], target) {
            list.push(doc.clone());
            continue;
        }
        if !read_alike(&owners.raws[i], &olds[i]) {
            return Err(WriteError::Bad(format!(
                "{} holds content the server ignores ({}); fix that file by hand first",
                doc.name,
                ignored(i, &olds[i], &owners, &base.list)
            )));
        }
        if let Some(text) = (i > 0).then(|| appended(&doc.text, target)).flatten() {
            list.push(Doc {
                name: doc.name.clone(),
                text,
            });
            continue;
        }
        let renamed = renames_in(&owners, &places, i, renames);
        let format = if i == 0 { main_format } else { Format::Yaml };
        let Patched { text, reformat: r } =
            patch::patch(&doc.text, format, target, &renamed).map_err(WriteError::Patch)?;
        reformat = reformat.or(r);
        list.push(Doc {
            name: doc.name.clone(),
            text,
        });
    }
    if let Some(gone) = moves.drops.iter().find(|d| base.get(d).is_none()) {
        return Err(not_a_file(gone));
    }
    Ok(Written {
        docs: Docs { list },
        reformat,
    })
}

fn still_holds(file: &str) -> WriteError {
    WriteError::Bad(format!("{file} cannot be deleted: it still holds items"))
}

fn not_a_file(file: &str) -> WriteError {
    WriteError::Bad(format!("{file} is not one of the config files"))
}

pub fn drop_edited(base: &Docs, edited: &Docs, drops: &[String]) -> Result<Docs, WriteError> {
    if let Some(gone) = drops.iter().find(|d| base.get(d).is_none()) {
        return Err(not_a_file(gone));
    }
    let main = &edited.main().name;
    if let Some(full) = drops
        .iter()
        .find(|d| *d == main || edited.text_of(d).is_some_and(|t| !empty_of_items(t)))
    {
        return Err(still_holds(full));
    }
    Ok(Docs {
        list: edited
            .list
            .iter()
            .filter(|d| !drops.contains(&d.name))
            .cloned()
            .collect(),
    })
}

pub fn note_moves(old: &Docs, applied: &crate::admin::core::ops::Applied, out: &mut Vec<Change>) {
    let owners = Owners::of(old);
    let name_of = |at: usize| {
        old.list
            .get(at)
            .map_or_else(|| old.main().name.clone(), |d| d.name.clone())
    };
    for (path, file) in &applied.places {
        let [section, name] = path.as_slice() else {
            continue;
        };
        let names = names_in(&owners, &applied.renames, section);
        let from = base_name(&names, name)
            .and_then(|first| owners.items.get(&(section.clone(), first)))
            .map(|at| name_of(*at));
        if from.as_ref().unwrap_or(&old.main().name) == file {
            continue;
        }
        let kind = burrow_config::drop_ins::sections()
            .into_iter()
            .find(|(s, _)| s == section)
            .map_or(ServiceEntry::ENTITY, |(_, k)| k);
        let known = out.iter().any(|c| c.path == *path);
        let to = if old.get(file).is_some() || file == &old.main().name {
            file.clone()
        } else {
            format!("a new file, {file}")
        };
        out.push(Change {
            kind: ChangeKind::Other,
            path: path.clone(),
            summary: match from {
                Some(from) => format!("moves {kind} {name} from {from} to {to}"),
                None => format!("adds {kind} {name} in {to}"),
            },
            part_of: known.then(|| path.clone()),
        });
    }
    for file in &applied.drops {
        out.push(Change {
            kind: ChangeKind::Other,
            path: Vec::new(),
            summary: format!("deletes {file}, which holds nothing any more"),
            part_of: None,
        });
    }
}

pub fn diffs(old: &Docs, new: &Docs, show: &dyn Fn(&str, &str, &str) -> Diff) -> Vec<FileDiff> {
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
                    show(name, a.unwrap_or(""), b.unwrap_or(""))
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
            assert!(
                burrow_config::drop_ins::new_name_ok("server.d", good),
                "{good}"
            );
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
            assert!(
                !burrow_config::drop_ins::new_name_ok("server.d", bad),
                "{bad}"
            );
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
