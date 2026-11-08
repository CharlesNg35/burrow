#![cfg(test)]

use std::collections::{BTreeMap, BTreeSet};

use burrow_admin_proto::ops::Op;
use serde_json::{Value, json};

use super::check::merged_of;
use super::{Item, MAIN, NAMES, Rng, SECTIONS, file_items};
use crate::wire::secret::Secret;

#[derive(Clone, Debug)]
pub(super) struct Track {
    pub(super) orig: Option<String>,
    pub(super) orig_name: Option<String>,
    pub(super) placed: Option<String>,
    pub(super) also_ok: Option<String>,
}

pub(super) struct Model {
    pub(super) items: BTreeMap<Item, Track>,
    files: Vec<String>,
    pub(super) drops: Vec<String>,
}

pub(super) fn locations(files: &BTreeMap<String, String>) -> BTreeMap<Item, Vec<String>> {
    let mut out: BTreeMap<Item, Vec<String>> = BTreeMap::new();
    for (f, t) in files {
        for (it, _) in file_items(t).unwrap_or_default() {
            out.entry(it).or_default().push(f.clone());
        }
    }
    out
}

fn model_of(files: &BTreeMap<String, String>) -> Model {
    let mut items = BTreeMap::new();
    for (it, at) in locations(files) {
        items.insert(
            it.clone(),
            Track {
                orig: Some(at[0].clone()),
                orig_name: Some(it.1.clone()),
                placed: None,
                also_ok: None,
            },
        );
    }
    Model {
        items,
        files: files.keys().cloned().collect(),
        drops: Vec::new(),
    }
}

pub(super) fn simulate(start: &BTreeMap<String, String>, ops: &[Op]) -> Model {
    let mut m = model_of(start);
    let orig_owner: BTreeMap<Item, String> = m
        .items
        .iter()
        .map(|(k, t)| (k.clone(), t.orig.clone().unwrap()))
        .collect();
    for op in ops {
        match op {
            Op::Add { path, .. } if path.len() == 2 => {
                let k = (path[0].clone(), path[1].clone());
                m.items.insert(
                    k.clone(),
                    Track {
                        orig: None,
                        orig_name: None,
                        placed: None,
                        also_ok: orig_owner.get(&k).cloned(),
                    },
                );
            }
            Op::Remove { path } if path.len() == 2 => {
                m.items.remove(&(path[0].clone(), path[1].clone()));
            }
            Op::Rename { path, to, .. } if path.len() == 2 => {
                if let Some(t) = m.items.remove(&(path[0].clone(), path[1].clone())) {
                    m.items.insert((path[0].clone(), to.clone()), t);
                }
            }
            Op::Place { path, file } => {
                if let Some(t) = m.items.get_mut(&(path[0].clone(), path[1].clone())) {
                    t.placed = Some(file.clone());
                }
                if !m.files.contains(file) {
                    m.files.push(file.clone());
                }
            }
            Op::DropFile { file } => m.drops.push(file.clone()),
            _ => {}
        }
    }
    m
}

impl Track {
    pub(super) fn expected(&self) -> String {
        self.placed
            .clone()
            .or(self.orig.clone())
            .unwrap_or_else(|| MAIN.to_string())
    }
}

type Names = BTreeMap<&'static str, Vec<String>>;

struct Gen<'a> {
    r: &'a mut Rng,
    files: &'a BTreeMap<String, String>,
    merged: Value,
    names: Names,
    binds: BTreeSet<u64>,
    file_list: Vec<String>,
    newfile: &'a mut usize,
    ops: Vec<Op>,
}

fn path(sec: &str, name: &str) -> Vec<String> {
    vec![sec.to_string(), name.to_string()]
}

impl Gen<'_> {
    fn fresh_name(&mut self) -> String {
        loop {
            let c = format!("{}{}", NAMES[self.r.below(NAMES.len())], self.r.below(4));
            if !self.names.values().any(|v| v.contains(&c)) && c != "home" && c != "ops" {
                return c;
            }
        }
    }

    fn next_bind(&mut self) -> u64 {
        let mut b = 9000;
        while self.binds.contains(&b) {
            b += 1;
        }
        self.binds.insert(b);
        b
    }

    fn movable(&self, sec: &str) -> Vec<String> {
        self.names[sec]
            .iter()
            .filter(|n| *n != "ops")
            .cloned()
            .collect()
    }

    fn some_file(&mut self, known_pct: u64) -> String {
        if self.r.chance(known_pct) && self.file_list.len() > 1 {
            return self.r.pick(&self.file_list).unwrap().clone();
        }
        *self.newfile += 1;
        let f = format!("server.d/n{}.yaml", *self.newfile);
        self.file_list.push(f.clone());
        f
    }

    fn renamed(&mut self, sec: &'static str, from: &str, to: &str, refs: bool) {
        self.ops.push(Op::Rename {
            path: path(sec, from),
            to: to.to_string(),
            refs,
        });
        let v = self.names.get_mut(sec).unwrap();
        v.retain(|n| n != from);
        v.push(to.to_string());
    }

    fn set_field(&mut self) {
        let Some(x) = self.r.pick(&self.names["services"]).cloned() else {
            return;
        };
        let (key, value) = if self.r.chance(60) {
            ("target", json!(3000 + self.r.below(900)))
        } else {
            ("bind", json!(self.next_bind()))
        };
        self.ops.push(Op::Set {
            path: vec!["services".into(), x, key.into()],
            value,
        });
    }

    fn set_reach(&mut self) {
        let visitors = self.movable("visitors");
        let v = self.r.pick(&visitors).cloned();
        let c = self.r.pick(&self.names["clients"]).cloned();
        if let (Some(v), Some(c)) = (v, c) {
            self.ops.push(Op::Set {
                path: vec!["visitors".into(), v, "reach".into()],
                value: json!({ c: "any" }),
            });
        }
    }

    fn add(&mut self, sec: &'static str) {
        let name = self.fresh_name();
        let value = match sec {
            "services" => {
                let c = self.r.pick(&self.names["clients"]).cloned();
                let bind = self.next_bind();
                let target = 3000 + self.r.below(900);
                json!({"client": c.unwrap_or("home".into()), "bind": bind, "target": target})
            }
            "clients" => json!({"secret": Secret::generate().encode()}),
            _ => json!({"secret": Secret::generate().encode(), "reach": {"home": "any"}}),
        };
        self.ops.push(Op::Add {
            path: path(sec, &name),
            value,
        });
        self.names.get_mut(sec).unwrap().push(name.clone());
        if self.r.chance(40) {
            let file = self.some_file(50);
            self.ops.push(Op::Place {
                path: path(sec, &name),
                file,
            });
        }
    }

    fn remove(&mut self, sec: &'static str) {
        let mut pool = self.movable(sec);
        if sec == "clients" && self.r.chance(80) {
            let all = serde_json::to_string(&self.merged).unwrap();
            pool.retain(|c| c != "home" && !all.contains(&format!("\"{c}\"")));
        }
        if let Some(x) = self.r.pick(&pool).cloned() {
            self.ops.push(Op::Remove {
                path: path(sec, &x),
            });
            self.names.get_mut(sec).unwrap().retain(|n| *n != x);
        }
    }

    fn rename(&mut self, sec: &'static str) {
        let Some(x) = self.r.pick(&self.movable(sec)).cloned() else {
            return;
        };
        let to = self.fresh_name();
        let refs = self.r.chance(85);
        self.renamed(sec, &x, &to, refs);
        match self.r.below(4) {
            0 => {
                let next = self.fresh_name();
                self.renamed(sec, &to, &next, refs);
            }
            1 => self.renamed(sec, &to, &x, refs),
            _ => {}
        }
    }

    fn swap(&mut self, sec: &'static str) {
        let pool = self.movable(sec);
        if pool.len() < 2 {
            return;
        }
        let a = self.r.pick(&pool).unwrap().clone();
        let b = loop {
            let b = self.r.pick(&pool).unwrap().clone();
            if b != a {
                break b;
            }
        };
        let tmp = self.fresh_name();
        let refs = self.r.chance(85);
        for (f, t) in [(&a, &tmp), (&b, &a), (&tmp, &b)] {
            self.ops.push(Op::Rename {
                path: path(sec, f),
                to: t.clone(),
                refs,
            });
        }
    }

    fn place(&mut self, sec: &'static str) {
        if let Some(x) = self.r.pick(&self.movable(sec)).cloned() {
            let file = self.some_file(65);
            self.ops.push(Op::Place {
                path: path(sec, &x),
                file,
            });
        }
    }

    fn dropped(&self, f: &str) -> bool {
        self.ops
            .iter()
            .any(|o| matches!(o, Op::DropFile { file } if file == f))
    }

    fn drop_file(&mut self) {
        let m = simulate(self.files, &self.ops);
        let mut empty: Vec<String> = self
            .files
            .keys()
            .filter(|f| *f != MAIN)
            .filter(|f| !m.items.values().any(|t| t.expected() == **f))
            .filter(|f| !self.dropped(f))
            .cloned()
            .collect();
        if empty.is_empty() && self.r.chance(20) {
            empty = self.files.keys().filter(|f| *f != MAIN).cloned().collect();
        }
        if let Some(f) = self.r.pick(&empty).cloned() {
            self.ops.push(Op::DropFile { file: f });
        }
    }

    fn drop_emptied(&mut self) {
        let m = simulate(self.files, &self.ops);
        for f in self.files.keys().filter(|f| *f != MAIN) {
            let empty = !m.items.values().any(|t| t.expected() == *f);
            let had = file_items(&self.files[f]).is_some_and(|i| !i.is_empty());
            if empty && had && !self.dropped(f) {
                self.ops.push(Op::DropFile { file: f.clone() });
            }
        }
    }

    fn one(&mut self) {
        let sec = match self.r.below(10) {
            0..=5 => "services",
            6..=7 => "clients",
            _ => "visitors",
        };
        match self.r.below(100) {
            0..=14 => self.set_field(),
            15..=19 => self.set_reach(),
            20..=31 => self.add(sec),
            32..=41 => self.remove(sec),
            42..=59 => self.rename(sec),
            60..=67 => self.swap(sec),
            68..=89 => self.place(sec),
            _ => self.drop_file(),
        }
    }
}

fn names_of(merged: &Value) -> (Names, BTreeSet<u64>) {
    let mut names = Names::new();
    let mut binds = BTreeSet::new();
    for s in SECTIONS {
        let items = merged.get(s).and_then(Value::as_object);
        names.insert(
            s,
            items
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default(),
        );
        if s == "services" {
            let found = items.into_iter().flatten();
            binds.extend(found.filter_map(|(_, v)| v.get("bind").and_then(Value::as_u64)));
        }
    }
    (names, binds)
}

pub(super) fn gen_change(
    r: &mut Rng,
    files: &BTreeMap<String, String>,
    newfile: &mut usize,
) -> Vec<Op> {
    let merged = merged_of(files);
    let (names, binds) = names_of(&merged);
    let mut g = Gen {
        r,
        files,
        merged,
        names,
        binds,
        file_list: files.keys().cloned().collect(),
        newfile,
        ops: Vec::new(),
    };
    for _ in 0..1 + g.r.below(6) {
        g.one();
    }
    if g.r.chance(50) {
        g.drop_emptied();
    }
    g.ops
}

pub(super) fn ui_order(ops: Vec<Op>) -> Vec<Op> {
    let mut rest = Vec::new();
    let mut places: Vec<(Vec<String>, String)> = Vec::new();
    let mut drops = Vec::new();
    for op in ops {
        match op {
            Op::Place { path, file } => {
                places.retain(|(p, _)| *p != path);
                places.push((path, file));
            }
            Op::DropFile { file } => drops.push(Op::DropFile { file }),
            Op::Rename { path, to, refs } => {
                for (p, _) in places.iter_mut() {
                    if *p == path {
                        p[1] = to.clone();
                    }
                }
                rest.push(Op::Rename { path, to, refs });
            }
            Op::Remove { path } => {
                places.retain(|(p, _)| *p != path);
                rest.push(Op::Remove { path });
            }
            o => rest.push(o),
        }
    }
    rest.extend(
        places
            .into_iter()
            .map(|(path, file)| Op::Place { path, file }),
    );
    rest.extend(drops);
    rest
}
