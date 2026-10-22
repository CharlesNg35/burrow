use burrow_admin_proto::Path;
use burrow_admin_proto::ops::Op;
use burrow_admin_proto::serde_json::{Map, Value, json};
use burrow_admin_proto::value::{self, as_secret};

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

fn special(v: &Value) -> bool {
    value::is_special(v) || as_new_secret(v).is_some()
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
                        out.push(Op::Add {
                            path: p,
                            value: cleaned,
                        });
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
                if rename_key(&mut v, path, to) && *refs && path.len() == 2 {
                    burrow_config::refs::rename_refs(&mut v, &path[0], &path[1], to);
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
            Op::ReplaceDocument { .. } | Op::SetText { .. } => {}
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
        if path.len() != 2 || !rename_key(&mut self.draft, path, to) {
            return false;
        }
        burrow_config::refs::rename_refs(&mut self.draft, &path[0], &path[1], to);
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
                burrow_config::refs::rename_refs(&mut renamed, &path[0], &path[1], to);
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
                from.first().map(String::as_str) == Some(section)
                    && from.get(1).map(String::as_str) == Some(name)
            })
            .map(|(_, to)| to.clone())
    }

    pub fn renamed_from(&self, section: &str, name: &str) -> Option<String> {
        self.renames
            .iter()
            .find(|(from, to)| from.first().map(String::as_str) == Some(section) && to == name)
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
mod tests {
    use super::*;

    fn p(s: &[&str]) -> Path {
        s.iter().map(|x| x.to_string()).collect()
    }

    fn secret(fp: &str) -> Value {
        json!({"$secret": {"fp": fp, "src": "literal"}})
    }

    fn base() -> Value {
        json!({
            "listen": 7835,
            "clients": {"home": {"secret": secret("aaaa0001")}},
            "visitors": {"ops": {"secret": secret("bbbb0002"), "admin": "write"}},
            "services": {"web": {"client": "home", "bind": 7840, "target": 3000}}
        })
    }

    #[test]
    fn edits_become_minimal_ops() {
        let mut c = Changeset::new(base());
        assert!(!c.dirty());
        c.set(&p(&["services", "web", "bind"]), json!("127.0.0.1:7841"));
        c.set(
            &p(&["services", "ssh"]),
            json!({"client": "home", "target": 22}),
        );
        c.remove(&p(&["listen"]));
        let ops = c.ops();
        assert_eq!(
            ops,
            vec![
                Op::Set {
                    path: p(&["services", "web", "bind"]),
                    value: json!("127.0.0.1:7841")
                },
                Op::Add {
                    path: p(&["services", "ssh"]),
                    value: json!({"client": "home", "target": 22})
                },
                Op::Remove {
                    path: p(&["listen"])
                },
            ]
        );
        assert!(c.modified(&p(&["services", "web", "bind"])));
        assert!(!c.modified(&p(&["services", "web", "target"])));
        assert_eq!(c.changed_entities(), 3);
        c.reset();
        assert!(!c.dirty());
    }

    #[test]
    fn removing_the_last_entry_of_a_new_group_leaves_no_empty_group() {
        let mut c = Changeset::new(json!({"visitors": {"lap": {"secret": "x"}}}));
        c.set(&p(&["visitors", "lap", "reach", "home"]), json!("any"));
        assert!(c.dirty());
        c.remove_pruned(&p(&["visitors", "lap", "reach", "home"]));
        assert!(!c.dirty(), "{:?}", c.ops());
        c.set(&p(&["pool", "min_idle"]), json!(8));
        c.remove_pruned(&p(&["pool", "min_idle"]));
        assert!(!c.dirty(), "{:?}", c.ops());
        c.set(&p(&["health_check", "interval_secs"]), json!(5));
        c.set(&p(&["health_check", "interval_secs"]), Value::Null);
        assert!(c.dirty(), "clearing a field keeps a group set on purpose");
        let mut kept = Changeset::new(json!({"pool": {"min_idle": 2}}));
        kept.remove_pruned(&p(&["pool", "min_idle"]));
        assert_eq!(
            kept.draft(),
            &json!({"pool": {}}),
            "a group the base has stays"
        );
    }

    #[test]
    fn a_new_client_added_to_services_counts_as_one_change() {
        let mut c = Changeset::new(json!({"clients": {"home": {}}, "services": {
            "web": {"client": "home", "target": 80},
            "db": {"client": "home", "target": 5432}
        }}));
        c.set(&p(&["clients", "example"]), json!({}));
        c.set(&p(&["services", "web", "client"]), Value::Null);
        c.set(
            &p(&["services", "web", "clients"]),
            json!(["home", "example"]),
        );
        c.set(&p(&["services", "db", "client"]), Value::Null);
        c.set(
            &p(&["services", "db", "clients"]),
            json!(["home", "example"]),
        );
        assert_eq!(c.changed_entities(), 1);
        c.set(&p(&["services", "db", "target"]), json!(5433));
        assert_eq!(c.changed_entities(), 2);
        c.rename(&p(&["clients", "home"]), "house");
        assert_eq!(c.changed_entities(), 4, "web now carries two changes");
        assert_eq!(c.renamed_from("clients", "house").as_deref(), Some("home"));
        assert_eq!(c.renamed_from("visitors", "house"), None);
        assert_eq!(c.renamed_to("clients", "home").as_deref(), Some("house"));
    }

    #[test]
    fn secrets_travel_as_handles_and_fingerprints() {
        let mut c = Changeset::new(base());
        c.set(&p(&["clients", "office"]), json!({}));
        c.add_secret(&p(&["clients", "office", "secret"]), "s1", "cccc0003");
        c.add_secret(&p(&["clients", "home", "secret"]), "s2", "dddd0004");
        let ops = c.ops();
        assert_eq!(
            ops,
            vec![
                Op::Add {
                    path: p(&["clients", "office"]),
                    value: json!({})
                },
                Op::SecretAdd {
                    path: p(&["clients", "home", "secret"]),
                    handle: "s2".into()
                },
                Op::SecretAdd {
                    path: p(&["clients", "office", "secret"]),
                    handle: "s1".into()
                },
            ]
        );
        let mut r = Changeset::new(base());
        r.add_secret(&p(&["clients", "home", "secret"]), "s3", "eeee0005");
        r.remove_secret(&p(&["clients", "home", "secret"]), "aaaa0001");
        assert_eq!(
            r.ops(),
            vec![
                Op::SecretAdd {
                    path: p(&["clients", "home", "secret"]),
                    handle: "s3".into()
                },
                Op::SecretRemove {
                    path: p(&["clients", "home", "secret"]),
                    fp: "aaaa0001".into()
                },
            ]
        );
        assert!(!format!("{:?}", r.ops()).contains("$secret"));
    }

    #[test]
    fn a_rename_is_one_op_and_carries_references() {
        let mut c = Changeset::new(base());
        assert!(c.rename(&p(&["clients", "home"]), "house"));
        assert_eq!(c.draft()["services"]["web"]["client"], "house");
        c.set(&p(&["services", "web", "target"]), json!(3001));
        let ops = c.ops();
        assert_eq!(
            ops[0],
            Op::Rename {
                path: p(&["clients", "home"]),
                to: "house".into(),
                refs: true
            }
        );
        assert_eq!(
            ops[1],
            Op::Set {
                path: p(&["services", "web", "target"]),
                value: json!(3001)
            }
        );
        assert_eq!(ops.len(), 2);
        assert!(!c.rename(&p(&["clients", "nope"]), "x"));
    }

    #[test]
    fn a_kept_draft_moves_onto_a_new_base_without_undoing_hand_edits() {
        let mut c = Changeset::new(base());
        c.set(&p(&["services", "web", "target"]), json!(3002));
        c.add_secret(&p(&["clients", "home", "secret"]), "s1", "ffff0006");
        let mut newer = base();
        newer["clients"]["office"] = json!({"secret": secret("9999aaaa")});
        c.rebase(newer.clone());
        assert_eq!(c.base(), &newer);
        assert_eq!(
            c.draft()["clients"]["office"],
            newer["clients"]["office"],
            "the hand edit stays"
        );
        let ops = c.ops();
        assert_eq!(ops.len(), 2, "{ops:?}");
        assert!(ops.contains(&Op::Set {
            path: p(&["services", "web", "target"]),
            value: json!(3002)
        }));
        assert!(ops.contains(&Op::SecretAdd {
            path: p(&["clients", "home", "secret"]),
            handle: "s1".into()
        }));
        let markers = c.draft()["clients"]["home"]["secret"].to_string();
        assert!(markers.contains("ffff0006"), "{markers}");
    }

    #[test]
    fn environment_values_are_left_alone_unless_replaced() {
        let mut b = base();
        b["listen"] = json!({"$env": {"raw": "${PORT}", "vars": ["PORT"]}});
        let mut c = Changeset::new(b);
        c.set(&p(&["services", "web", "target"]), json!(1));
        assert_eq!(c.ops().len(), 1);
        c.set(&p(&["listen"]), json!(7836));
        assert!(c.ops().contains(&Op::Set {
            path: p(&["listen"]),
            value: json!(7836)
        }));
    }

    struct Rng(u64);

    impl Rng {
        fn next(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n
        }
    }

    fn random_value(r: &mut Rng, depth: u32) -> Value {
        match r.next(if depth == 0 { 4 } else { 6 }) {
            0 => json!(r.next(100)),
            1 => json!(["a", "b", "c"][r.next(3) as usize]),
            2 => json!(r.next(2) == 0),
            3 => json!([r.next(9), r.next(9)]),
            _ => {
                let mut m = Map::new();
                for _ in 0..r.next(4) {
                    m.insert(format!("k{}", r.next(5)), random_value(r, depth - 1));
                }
                Value::Object(m)
            }
        }
    }

    fn random_path(r: &mut Rng) -> Path {
        (0..=r.next(3)).map(|_| format!("k{}", r.next(5))).collect()
    }

    #[test]
    fn replaying_the_ops_on_the_base_gives_the_draft() {
        let cases: u64 = std::env::var("BURROW_CHANGESET_CASES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3000);
        let mut r = Rng(0x9e37_79b9_7f4a_7c15);
        for case in 0..cases {
            let base = Value::Object(match random_value(&mut r, 3) {
                Value::Object(m) => m,
                other => Map::from_iter([("k0".to_string(), other)]),
            });
            let mut c = Changeset::new(base.clone());
            for _ in 0..=r.next(5) {
                let path = random_path(&mut r);
                if r.next(3) == 0 {
                    c.remove(&path);
                } else {
                    c.set(&path, random_value(&mut r, 2));
                }
            }
            let ops = c.ops();
            assert_eq!(
                &apply(&base, &ops),
                c.draft(),
                "case {case}: base {base} ops {ops:?}"
            );
            assert_eq!(ops.is_empty(), &base == c.draft(), "case {case}");
        }
    }

    #[test]
    fn ops_between_counts_field_changes() {
        let a = json!({"bind": 1, "target": 2, "allow_from": ["x"]});
        let b = json!({"bind": 1, "target": 3, "allow_from": ["x", "y"], "mode": "balance"});
        let ops = ops_between(&a, &b);
        assert_eq!(ops.len(), 3, "{ops:?}");
        assert!(ops_between(&a, &a).is_empty());
    }

    #[test]
    fn a_renamed_client_keeps_its_place_in_the_draft_reach() {
        let mut c = Changeset::new(json!({
            "clients": {"home": {}, "office": {}},
            "visitors": {"lap": {"reach": {"home": "any", "office": "any"}}}
        }));
        assert!(c.rename(&["clients".to_string(), "home".to_string()], "home2"));
        let keys: Vec<&String> = c.draft()["visitors"]["lap"]["reach"]
            .as_object()
            .unwrap()
            .keys()
            .collect();
        assert_eq!(keys, ["home2", "office"]);
    }
}
