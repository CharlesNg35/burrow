use burrow_admin_proto::Path;
use burrow_admin_proto::ops::Op;
use serde_json::{Map, Value};

use super::values::{fingerprint_of, has_secret_marker, raw_of};
use crate::config::env::EnvSource;
use crate::config::patch::Rename;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpError(pub String);

pub struct Fresh {
    pub handle: String,
    pub path: Path,
    pub value: String,
    pub fp: String,
}

pub struct Applied {
    pub target: Value,
    pub renames: Vec<Rename>,
    pub fresh: Vec<Fresh>,
    pub document: Option<String>,
}

pub trait Context {
    fn secret(&self, handle: &str) -> Option<(String, String)>;
    fn history(&self, rev: &str) -> Option<String>;
    fn env(&self) -> &dyn EnvSource;
}

fn err(msg: impl Into<String>) -> OpError {
    OpError(msg.into())
}

fn show(path: &[String]) -> String {
    path.join(".")
}

fn parent_mut<'a>(
    root: &'a mut Value,
    path: &[String],
    create: bool,
) -> Result<&'a mut Map<String, Value>, OpError> {
    let mut cur = root;
    for k in path {
        if !cur.is_object() {
            return Err(err(format!("{} is not a mapping", show(path))));
        }
        let map = cur.as_object_mut().expect("checked");
        if !map.contains_key(k) {
            if !create {
                return Err(err(format!("{} does not exist", show(path))));
            }
            map.insert(k.clone(), Value::Object(Map::new()));
        }
        cur = map.get_mut(k).expect("inserted");
        if cur.is_null() && create {
            *cur = Value::Object(Map::new());
        }
    }
    cur.as_object_mut()
        .ok_or_else(|| err(format!("{} is not a mapping", show(path))))
}

fn checked_value(v: &Value) -> Result<Value, OpError> {
    if has_secret_marker(v) {
        return Err(err(
            "a value may not contain $secret; use secret_add with a generated secret",
        ));
    }
    Ok(raw_of(v))
}

fn path_of(op: &Op) -> Option<&Path> {
    match op {
        Op::Set { path, .. }
        | Op::Add { path, .. }
        | Op::Remove { path }
        | Op::Rename { path, .. }
        | Op::SecretAdd { path, .. }
        | Op::SecretRemove { path, .. }
        | Op::SetText { path, .. } => Some(path),
        Op::ReplaceDocument { .. } => None,
    }
}

pub fn apply(raw: &Value, ops: &[Op], cx: &dyn Context) -> Result<Applied, OpError> {
    let max = crate::config::tree::MAX_DEPTH;
    if let Some(path) = ops.iter().filter_map(path_of).find(|p| p.len() > max) {
        return Err(err(format!(
            "a path of {} keys is nested more than {max} levels deep; a config needs far fewer",
            path.len()
        )));
    }
    let mut target = raw.clone();
    let mut renames = Vec::new();
    let mut fresh = Vec::new();
    let mut document = None;
    for op in ops {
        match op {
            Op::Set { path, value } => {
                let (last, parents) = path.split_last().ok_or_else(|| err("set needs a path"))?;
                let value = checked_value(value)?;
                parent_mut(&mut target, parents, true)?.insert(last.clone(), value);
            }
            Op::Add { path, value } => {
                let (last, parents) = path.split_last().ok_or_else(|| err("add needs a path"))?;
                let value = checked_value(value)?;
                let map = parent_mut(&mut target, parents, true)?;
                if map.contains_key(last) {
                    return Err(err(format!("{} already exists", show(path))));
                }
                map.insert(last.clone(), value);
            }
            Op::Remove { path } => {
                let (last, parents) = path
                    .split_last()
                    .ok_or_else(|| err("remove needs a path"))?;
                let map = parent_mut(&mut target, parents, false)?;
                if map.shift_remove(last).is_none() {
                    return Err(err(format!("{} does not exist", show(path))));
                }
            }
            Op::Rename { path, to, refs } => {
                rename_entry(&mut target, path, to, *refs, &mut renames)?;
            }
            Op::SecretAdd { path, handle } => {
                add_secret(&mut target, path, handle, cx, &mut fresh)?;
            }
            Op::SecretRemove { path, fp } => {
                remove_secret(&mut target, path, fp, cx)?;
            }
            Op::ReplaceDocument { from_history } => {
                let text = cx
                    .history(from_history)
                    .ok_or_else(|| err(format!("version {from_history} is not in the history")))?;
                let (_, _, src) = crate::config::patch::split_text(&text);
                target = crate::config::tree::parse(&src)
                    .map_err(|_| err("that version does not parse"))?
                    .to_json();
                document = Some(text);
            }
            Op::SetText { path, text } => {
                let (last, parents) = path
                    .split_last()
                    .ok_or_else(|| err("set_text needs a path"))?;
                let parsed = crate::config::tree::parse(text)
                    .map_err(|e| {
                        err(format!(
                            "{}: {}",
                            show(path),
                            e.first().map(|e| e.message.clone()).unwrap_or_default()
                        ))
                    })?
                    .to_json();
                parent_mut(&mut target, parents, true)?.insert(last.clone(), parsed);
            }
        }
    }
    if document.is_some() && ops.len() > 1 {
        return Err(err("replace_document must be the only operation"));
    }
    Ok(Applied {
        target,
        renames,
        fresh,
        document,
    })
}

fn rename_entry(
    target: &mut Value,
    path: &[String],
    to: &str,
    refs: bool,
    renames: &mut Vec<Rename>,
) -> Result<(), OpError> {
    let (last, parents) = path
        .split_last()
        .ok_or_else(|| err("rename needs a path"))?;
    if !burrow_syntax::name::valid_name(to) {
        return Err(err(format!("\"{to}\" is not a valid name")));
    }
    let map = parent_mut(target, parents, false)?;
    if map.contains_key(to) {
        return Err(err(format!("{} already exists", to)));
    }
    if !map.contains_key(last) {
        return Err(err(format!("{} does not exist", show(path))));
    }
    let entries = std::mem::take(map);
    for (k, v) in entries {
        let k = if k == *last { to.to_string() } else { k };
        map.insert(k, v);
    }
    renames.push(Rename {
        path: path.to_vec(),
        to: to.to_string(),
    });
    if refs && parents.len() == 1 {
        for key in burrow_config::refs::rename_refs(target, &parents[0], last, to) {
            renames.push(Rename {
                path: key,
                to: to.to_string(),
            });
        }
    }
    Ok(())
}

fn add_secret(
    target: &mut Value,
    path: &[String],
    handle: &str,
    cx: &dyn Context,
    fresh: &mut Vec<Fresh>,
) -> Result<(), OpError> {
    let (value, fp) = cx
        .secret(handle)
        .ok_or_else(|| err(format!("secret handle {handle} is unknown or expired")))?;
    let (last, parents) = path
        .split_last()
        .ok_or_else(|| err("secret_add needs a path"))?;
    let map = parent_mut(target, parents, true)?;
    let next = match map.shift_remove(last) {
        None | Some(Value::Null) => Value::String(value.clone()),
        Some(Value::String(old)) => {
            Value::Array(vec![Value::String(old), Value::String(value.clone())])
        }
        Some(Value::Array(mut items)) => {
            items.push(Value::String(value.clone()));
            Value::Array(items)
        }
        Some(_) => return Err(err(format!("{} is not a secret", show(path)))),
    };
    map.insert(last.clone(), next);
    fresh.push(Fresh {
        handle: handle.to_string(),
        path: path.to_vec(),
        value,
        fp,
    });
    Ok(())
}

fn remove_secret(
    target: &mut Value,
    path: &[String],
    fp: &str,
    cx: &dyn Context,
) -> Result<(), OpError> {
    let (last, parents) = path
        .split_last()
        .ok_or_else(|| err("secret_remove needs a path"))?;
    let map = parent_mut(target, parents, false)?;
    let items: Vec<String> = match map.get(last) {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => return Err(err(format!("{} is not a secret", show(path)))),
    };
    let at = items
        .iter()
        .position(|s| fingerprint_of(s, cx.env()).as_deref() == Some(fp))
        .ok_or_else(|| err(format!("{} has no secret {fp}", show(path))))?;
    if items.len() == 1 {
        return Err(err(
            "the last secret cannot be removed; remove the entry instead",
        ));
    }
    let mut rest = items;
    rest.remove(at);
    let next = if rest.len() == 1 {
        Value::String(rest.remove(0))
    } else {
        Value::Array(rest.into_iter().map(Value::String).collect())
    };
    map.insert(last.clone(), next);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Cx(crate::wire::secret::Secret);

    impl Context for Cx {
        fn secret(&self, handle: &str) -> Option<(String, String)> {
            (handle == "s1").then(|| (self.0.encode(), self.0.fingerprint()))
        }
        fn history(&self, rev: &str) -> Option<String> {
            (rev == "b2s:old").then(|| "listen: 1\n".to_string())
        }
        fn env(&self) -> &dyn EnvSource {
            &|_: &str| None
        }
    }

    fn p(s: &[&str]) -> Path {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn ops_edit_the_raw_tree() {
        let cx = Cx(crate::wire::secret::Secret::generate());
        let old = crate::wire::secret::Secret::generate();
        let raw = json!({"clients": {"home": {"secret": old.encode()}}, "services": {"web": {"client": "home", "target": 1}}, "visitors": {"v": {"secret": "x", "reach": {"home": "any"}}}});
        let out = apply(
            &raw,
            &[
                Op::Set {
                    path: p(&["services", "web", "bind"]),
                    value: json!({"$env": {"raw": "${P}"}}),
                },
                Op::SecretAdd {
                    path: p(&["clients", "home", "secret"]),
                    handle: "s1".into(),
                },
                Op::Rename {
                    path: p(&["clients", "home"]),
                    to: "house".into(),
                    refs: true,
                },
                Op::Add {
                    path: p(&["services", "ssh"]),
                    value: json!({"client": "house", "target": 22}),
                },
            ],
            &cx,
        )
        .unwrap();
        assert_eq!(out.target["services"]["web"]["bind"], "${P}");
        assert_eq!(out.target["services"]["web"]["client"], "house");
        assert_eq!(out.target["visitors"]["v"]["reach"]["house"], "any");
        assert_eq!(
            out.target["clients"]["house"]["secret"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(out.fresh[0].fp, cx.0.fingerprint());
        let renamed: Vec<(Vec<String>, &str)> = out
            .renames
            .iter()
            .map(|r| (r.path.clone(), r.to.as_str()))
            .collect();
        assert_eq!(
            renamed,
            [
                (p(&["clients", "home"]), "house"),
                (p(&["visitors", "v", "reach", "home"]), "house"),
            ]
        );
        let removed = apply(
            &out.target,
            &[Op::SecretRemove {
                path: p(&["clients", "house", "secret"]),
                fp: old.fingerprint(),
            }],
            &cx,
        )
        .unwrap();
        assert_eq!(removed.target["clients"]["house"]["secret"], cx.0.encode());
        assert!(
            apply(
                &removed.target,
                &[Op::SecretRemove {
                    path: p(&["clients", "house", "secret"]),
                    fp: cx.0.fingerprint()
                }],
                &cx
            )
            .is_err()
        );
        assert!(
            apply(
                &raw,
                &[Op::Set {
                    path: p(&["a"]),
                    value: json!({"$secret": {"fp": "x", "src": "literal"}})
                }],
                &cx
            )
            .is_err()
        );
        assert!(apply(&raw, &[Op::Remove { path: p(&["nope"]) }], &cx).is_err());
        let doc = apply(
            &raw,
            &[Op::ReplaceDocument {
                from_history: "b2s:old".into(),
            }],
            &cx,
        )
        .unwrap();
        assert_eq!(doc.target, json!({"listen": 1}));
        assert!(doc.document.is_some());
    }

    #[test]
    fn a_path_deeper_than_a_config_is_refused_before_the_tree_changes() {
        let run = |path: Path| {
            std::thread::Builder::new()
                .stack_size(256 * 1024)
                .spawn(move || {
                    let cx = Cx(crate::wire::secret::Secret::generate());
                    let ops = [
                        Op::Set {
                            path: path.clone(),
                            value: json!(1),
                        },
                        Op::Add {
                            path: path.clone(),
                            value: json!(1),
                        },
                        Op::Remove { path: path.clone() },
                        Op::SetText {
                            path,
                            text: "1".into(),
                        },
                    ];
                    ops.iter()
                        .map(|op| apply(&json!({}), std::slice::from_ref(op), &cx).map(|_| ()))
                        .collect::<Vec<_>>()
                })
                .unwrap()
                .join()
                .unwrap()
        };
        let deep: Path = (0..5000).map(|i| format!("k{i}")).collect();
        for r in run(deep) {
            let e = r.unwrap_err();
            assert!(e.0.contains("more than 64"), "{}", e.0);
        }
        let ok: Path = (0..crate::config::tree::MAX_DEPTH)
            .map(|i| format!("k{i}"))
            .collect();
        assert!(run(ok)[0].is_ok());
    }
}
