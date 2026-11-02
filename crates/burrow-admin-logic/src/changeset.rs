use burrow_admin_proto::Path;
use burrow_admin_proto::ops::Op;
use burrow_admin_proto::serde_json::{Map, Value, json};
use burrow_admin_proto::value::{self, as_secret};
use burrow_config::Entry;

pub const NEW_SECRET: &str = "$new_secret";

pub fn new_secret(handle: &str, fp: &str) -> Value {
    json!({ NEW_SECRET: { "handle": handle, "fp": fp } })
}

pub fn as_new_secret(v: &Value) -> Option<(String, String)> {
    let o = v.as_object()?.get(NEW_SECRET)?;
    Some((
        o.get("handle")?.as_str()?.to_string(),
        o.get("fp")?.as_str()?.to_string(),
    ))
}

pub const NEW_PASSWORD: &str = "$new_password";

pub fn new_password(handle: &str) -> Value {
    json!({ NEW_PASSWORD: { "handle": handle } })
}

pub fn as_new_password(v: &Value) -> Option<String> {
    let o = v.as_object()?.get(NEW_PASSWORD)?;
    Some(o.get("handle")?.as_str()?.to_string())
}

fn special(v: &Value) -> bool {
    value::is_special(v) || as_new_secret(v).is_some() || as_new_password(v).is_some()
}

fn secret_items(v: &Value) -> Option<Vec<&Value>> {
    let items: Vec<&Value> = match v {
        Value::Array(a) => a.iter().collect(),
        other => vec![other],
    };
    items
        .iter()
        .all(|i| as_secret(i).is_some() || as_new_secret(i).is_some())
        .then_some(items)
}

fn strip_new(v: &Value, path: &[String], adds: &mut Vec<Op>) -> Value {
    if let Some(handle) = as_new_password(v) {
        adds.push(Op::PasswordSet {
            path: path.to_vec(),
            handle,
        });
        return Value::Null;
    }
    if let Some(items) = secret_items(v)
        && items.iter().any(|i| as_new_secret(i).is_some())
    {
        for i in items {
            if let Some((handle, _)) = as_new_secret(i) {
                adds.push(Op::SecretAdd {
                    path: path.to_vec(),
                    handle,
                });
            }
        }
        return Value::Null;
    }
    match v {
        Value::Object(m) if !special(v) => {
            let mut out = Map::new();
            for (k, x) in m {
                let cleaned = strip_new(x, &value::child(path, k), adds);
                if !cleaned.is_null() || x.is_null() {
                    out.insert(k.clone(), cleaned);
                }
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

fn diff(base: &Value, draft: &Value, path: &[String], out: &mut Vec<Op>, tail: &mut Vec<Op>) {
    if base == draft {
        return;
    }
    if let Some(handle) = as_new_password(draft) {
        tail.push(Op::PasswordSet {
            path: path.to_vec(),
            handle,
        });
        return;
    }
    if let (Some(old), Some(new)) = (secret_items(base), secret_items(draft)) {
        let kept: Vec<String> = new
            .iter()
            .filter_map(|i| as_secret(i).and_then(|s| s.fp))
            .collect();
        for adds in new.iter().filter_map(|i| as_new_secret(i)) {
            tail.push(Op::SecretAdd {
                path: path.to_vec(),
                handle: adds.0,
            });
        }
        for gone in old
            .iter()
            .filter_map(|i| as_secret(i).and_then(|s| s.fp))
            .filter(|fp| !kept.contains(fp))
        {
            tail.push(Op::SecretRemove {
                path: path.to_vec(),
                fp: gone,
            });
        }
        return;
    }
    match (base, draft) {
        (Value::Object(b), Value::Object(d)) if !special(base) && !special(draft) => {
            for (k, dv) in d {
                let p = value::child(path, k);
                match b.get(k) {
                    None => {
                        let mut adds = Vec::new();
                        let cleaned = strip_new(dv, &p, &mut adds);
                        if !cleaned.is_null() || dv.is_null() {
                            out.push(Op::Add {
                                path: p,
                                value: cleaned,
                            });
                        }
                        tail.extend(adds);
                    }
                    Some(bv) => diff(bv, dv, &p, out, tail),
                }
            }
            for k in b.keys().filter(|k| !d.contains_key(*k)) {
                out.push(Op::Remove {
                    path: value::child(path, k),
                });
            }
        }
        _ => {
            let mut adds = Vec::new();
            let cleaned = strip_new(draft, path, &mut adds);
            out.push(Op::Set {
                path: path.to_vec(),
                value: cleaned,
            });
            tail.extend(adds);
        }
    }
}

pub fn ops_between(base: &Value, draft: &Value) -> Vec<Op> {
    let (mut main, mut tail) = (Vec::new(), Vec::new());
    diff(base, draft, &[], &mut main, &mut tail);
    main.extend(tail);
    main
}

fn parent_mut<'a>(root: &'a mut Value, path: &[String]) -> Option<&'a mut Map<String, Value>> {
    let mut cur = root;
    for k in path {
        if !cur.is_object() {
            return None;
        }
        let m = cur.as_object_mut()?;
        if !m.contains_key(k) {
            m.insert(k.clone(), Value::Object(Map::new()));
        }
        cur = m.get_mut(k)?;
        if cur.is_null() {
            *cur = Value::Object(Map::new());
        }
    }
    cur.as_object_mut()
}

pub fn rename_key(root: &mut Value, path: &[String], to: &str) -> bool {
    let Some((last, parents)) = path.split_last() else {
        return false;
    };
    let Some(map) = parent_mut(root, parents) else {
        return false;
    };
    if !map.contains_key(last) || map.contains_key(to) {
        return false;
    }
    let entries = std::mem::take(map);
    for (k, v) in entries {
        let k = if k == *last { to.to_string() } else { k };
        map.insert(k, v);
    }
    true
}

pub fn rename_under(snapshot: &mut Value, at: &[String], path: &[String], to: &str) {
    let Some(rel) = path.strip_prefix(at) else {
        return;
    };
    if rename_key(snapshot, rel, to) && at.is_empty() {
        burrow_config::refs::rename_refs(snapshot, path, to);
    }
}

pub fn apply(root: &Value, ops: &[Op]) -> Value {
    let mut v = root.clone();
    for op in ops {
        match op {
            Op::Set { path, value } | Op::Add { path, value } => {
                if let Some((last, parents)) = path.split_last()
                    && let Some(m) = parent_mut(&mut v, parents)
                {
                    m.insert(last.clone(), value.clone());
                }
            }
            Op::Remove { path } => {
                if let Some((last, parents)) = path.split_last()
                    && let Some(m) = parent_mut(&mut v, parents)
                {
                    m.shift_remove(last);
                }
            }
            Op::Rename { path, to, refs } => {
                if rename_key(&mut v, path, to) && *refs {
                    burrow_config::refs::rename_refs(&mut v, path, to);
                }
            }
            Op::SecretAdd { path, handle } => {
                if let Some((last, parents)) = path.split_last()
                    && let Some(m) = parent_mut(&mut v, parents)
                {
                    let marker = new_secret(handle, "");
                    let next = match m.shift_remove(last) {
                        None | Some(Value::Null) => marker,
                        Some(Value::Array(mut a)) => {
                            a.push(marker);
                            Value::Array(a)
                        }
                        Some(one) => Value::Array(vec![one, marker]),
                    };
                    m.insert(last.clone(), next);
                }
            }
            Op::SecretRemove { path, fp } => {
                if let Some(cur) = value::get_mut(&mut v, path) {
                    let items: Vec<Value> = match cur.take() {
                        Value::Array(a) => a,
                        one => vec![one],
                    };
                    let mut rest: Vec<Value> = items
                        .into_iter()
                        .filter(|i| as_secret(i).and_then(|s| s.fp).as_deref() != Some(fp.as_str()))
                        .collect();
                    *cur = if rest.len() == 1 {
                        rest.remove(0)
                    } else {
                        Value::Array(rest)
                    };
                }
            }
            Op::PasswordSet { path, handle } => {
                if let Some((last, parents)) = path.split_last()
                    && let Some(m) = parent_mut(&mut v, parents)
                {
                    m.insert(last.clone(), new_password(handle));
                }
            }
            Op::ReplaceDocument { .. }
            | Op::SetText { .. }
            | Op::Place { .. }
            | Op::DropFile { .. } => {}
        }
    }
    v
}

#[derive(Debug, Clone, PartialEq)]
pub struct Changeset {
    base: Value,
    draft: Value,
    renames: Vec<(Path, String)>,
    fresh: Vec<(String, String)>,
}

impl Changeset {
    pub fn new(base: Value) -> Self {
        Changeset {
            draft: base.clone(),
            base,
            renames: Vec::new(),
            fresh: Vec::new(),
        }
    }

    pub fn base(&self) -> &Value {
        &self.base
    }

    pub fn draft(&self) -> &Value {
        &self.draft
    }

    pub fn set(&mut self, path: &[String], value: Value) {
        value::put(&mut self.draft, path, (!value.is_null()).then_some(value));
    }

    pub fn remove_pruned(&mut self, path: &[String]) {
        self.set(path, Value::Null);
        for n in (1..path.len()).rev() {
            let at = &path[..n];
            let empty = match value::get(&self.draft, at) {
                Some(Value::Object(m)) => m.is_empty(),
                Some(Value::Array(a)) => a.is_empty(),
                _ => false,
            };
            if !empty || value::get(&self.base, at).is_some() {
                break;
            }
            value::put(&mut self.draft, at, None);
        }
    }

    pub fn remove(&mut self, path: &[String]) {
        self.set(path, Value::Null);
    }

    pub fn rename(&mut self, path: &[String], to: &str) -> bool {
        let entity = path
            .split_last()
            .and_then(|(_, parent)| burrow_config::entity_at(parent));
        if entity.is_none() || !rename_key(&mut self.draft, path, to) {
            return false;
        }
        burrow_config::refs::rename_refs(&mut self.draft, path, to);
        self.renames.push((path.to_vec(), to.to_string()));
        true
    }

    pub fn add_secret(&mut self, path: &[String], handle: &str, fp: &str) {
        let marker = new_secret(handle, fp);
        let next = match value::get(&self.draft, path).cloned() {
            None | Some(Value::Null) => marker,
            Some(Value::Array(mut a)) => {
                a.push(marker);
                Value::Array(a)
            }
            Some(one) => Value::Array(vec![one, marker]),
        };
        self.set(path, next);
        self.fresh.push((handle.to_string(), fp.to_string()));
    }

    pub fn set_password(&mut self, path: &[String], handle: &str) {
        self.set(path, new_password(handle));
    }

    pub fn remove_secret(&mut self, path: &[String], fp: &str) {
        let Some(cur) = value::get(&self.draft, path).cloned() else {
            return;
        };
        let items: Vec<Value> = match cur {
            Value::Array(a) => a,
            one => vec![one],
        };
        let mut rest: Vec<Value> = items
            .into_iter()
            .filter(|i| {
                as_secret(i).and_then(|s| s.fp).as_deref() != Some(fp)
                    && as_new_secret(i).map(|(_, f)| f).as_deref() != Some(fp)
            })
            .collect();
        let next = if rest.len() == 1 {
            rest.remove(0)
        } else {
            Value::Array(rest)
        };
        self.set(path, next);
    }

    pub fn ops(&self) -> Vec<Op> {
        let mut renamed = self.base.clone();
        let mut out = Vec::new();
        for (path, to) in &self.renames {
            if rename_key(&mut renamed, path, to) {
                burrow_config::refs::rename_refs(&mut renamed, path, to);
                out.push(Op::Rename {
                    path: path.clone(),
                    to: to.clone(),
                    refs: true,
                });
            }
        }
        let (mut main, mut tail) = (Vec::new(), Vec::new());
        diff(&renamed, &self.draft, &[], &mut main, &mut tail);
        out.extend(main);
        out.extend(tail);
        out
    }

    pub fn dirty(&self) -> bool {
        !self.ops().is_empty()
    }

    pub fn modified(&self, path: &[String]) -> bool {
        value::get(&self.base, path) != value::get(&self.draft, path)
    }

    pub fn reset(&mut self) {
        *self = Changeset::new(self.base.clone());
    }

    pub fn rebase(&mut self, base: Value) {
        let ops = self.ops();
        let mut draft = apply(&base, &ops);
        for (handle, fp) in &self.fresh {
            fix_fp(&mut draft, handle, fp);
        }
        self.base = base;
        self.draft = draft;
        self.renames.clear();
        for op in ops {
            if let Op::Rename { path, to, .. } = op {
                self.renames.push((path, to));
            }
        }
    }

    pub fn renamed_to(&self, section: &str, name: &str) -> Option<String> {
        self.renames
            .iter()
            .find(|(from, _)| {
                from.len() == 2
                    && from.first().map(String::as_str) == Some(section)
                    && from.get(1).map(String::as_str) == Some(name)
            })
            .map(|(_, to)| to.clone())
    }

    pub fn renamed_from(&self, section: &str, name: &str) -> Option<String> {
        self.renames
            .iter()
            .find(|(from, to)| {
                from.len() == 2 && from.first().map(String::as_str) == Some(section) && to == name
            })
            .and_then(|(from, _)| from.get(1).cloned())
    }

    pub fn fresh(&self) -> &[(String, String)] {
        &self.fresh
    }

    pub fn changed_entities(&self) -> usize {
        let mut paths: Vec<Path> = self
            .ops()
            .iter()
            .filter_map(|o| o.path().cloned())
            .map(|p| p.into_iter().take(2).collect::<Path>())
            .map(|p| {
                self.renames
                    .iter()
                    .find(|(from, to)| p.len() == 2 && from[0] == p[0] && *to == p[1])
                    .map(|(from, _)| from.clone())
                    .unwrap_or(p)
            })
            .filter(|p| {
                burrow_config::refs::part_of(&self.base, &self.draft, p, &self.renames).is_none()
            })
            .map(|p| {
                let entity = p.first().is_some_and(|s| {
                    [
                        burrow_config::ClientEntry::section(),
                        burrow_config::VisitorEntry::section(),
                        burrow_config::ServiceEntry::section(),
                    ]
                    .contains(&s.as_str())
                });
                if entity { p } else { Vec::new() }
            })
            .collect();
        paths.sort();
        paths.dedup();
        paths.len()
    }
}

fn fix_fp(v: &mut Value, handle: &str, fp: &str) {
    match v {
        Value::Object(m) => {
            if let Some(inner) = m.get_mut(NEW_SECRET).and_then(Value::as_object_mut)
                && inner.get("handle").and_then(Value::as_str) == Some(handle)
            {
                inner.insert("fp".into(), Value::String(fp.to_string()));
                return;
            }
            m.values_mut().for_each(|x| fix_fp(x, handle, fp));
        }
        Value::Array(a) => a.iter_mut().for_each(|x| fix_fp(x, handle, fp)),
        _ => {}
    }
}

#[cfg(test)]
mod tests;
