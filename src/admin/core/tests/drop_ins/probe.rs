//! A randomized probe of multi-file plans: random admin changes on a set with drop-ins, each checked against invariants.
//! Seeds are fixed; PROBE_SEED moves the first seed and PROBE_CASES sets how many run per order (default 80).
//! `model` generates changes and predicts where items land, `check` plans, applies and checks one change.

#![cfg(test)]

mod check;
mod model;

use std::collections::{BTreeMap, BTreeSet};

use burrow_admin_proto::ops::Op;
use serde_json::Value;

use super::Set;
use crate::admin::core::plan;
use crate::wire::secret::Secret;
use check::{Stats, run_change};
use model::{gen_change, ui_order};

const SECTIONS: [&str; 3] = ["services", "clients", "visitors"];
const MAIN: &str = "server.yaml";

struct Rng(u64);
impl Rng {
    fn step(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.step() % n.max(1) as u64) as usize
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.step() % 100 < pct
    }
    fn pick<'a, T>(&mut self, v: &'a [T]) -> Option<&'a T> {
        if v.is_empty() {
            None
        } else {
            Some(&v[self.below(v.len())])
        }
    }
}

const NAMES: [&str; 14] = [
    "alpha", "beta", "gamma", "delta", "eps", "zeta", "eta", "theta", "iota", "kappa", "lam", "mu",
    "nu", "xi",
];

struct Init {
    main: String,
    drop_ins: Vec<(String, String)>,
    secrets: Vec<String>,
}

fn init_for(seed: u64, set: &Set) -> Init {
    let mut r = Rng(seed ^ 0xA5A5_1234);
    let mut secrets = vec![set.w.home.encode(), set.w.ops.encode()];
    let mut id = 0;
    let mut port = 8000;
    let mut clients = vec!["home".to_string()];
    let mut visitors: Vec<String> = Vec::new();
    let fresh = |r: &mut Rng, used: &mut BTreeSet<String>| loop {
        let n = format!("{}{}", NAMES[r.below(NAMES.len())], r.below(4));
        if used.insert(n.clone()) {
            return n;
        }
    };
    let mut used: BTreeSet<String> = ["home", "ops", "web"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let n_files = r.below(4);
    let mut per_file: Vec<BTreeMap<&str, Vec<String>>> = vec![BTreeMap::new(); n_files + 1];
    let total = 2 + r.below(3 * (n_files + 1) + 1);
    for _ in 0..total {
        let f = r.below(n_files + 1);
        let sec = match r.below(10) {
            0..=5 => "services",
            6..=7 => "clients",
            _ => "visitors",
        };
        let name = fresh(&mut r, &mut used);
        id += 1;
        let tag = format!("# id{id}");
        let lines = match sec {
            "clients" => {
                let s = Secret::generate().encode();
                secrets.push(s.clone());
                clients.push(name.clone());
                if r.chance(50) {
                    format!("  {name}: {{ secret: {s} }} {tag}\n")
                } else {
                    format!("  {name}: {tag}\n    secret: {s}\n")
                }
            }
            "visitors" => {
                let s = Secret::generate().encode();
                secrets.push(s.clone());
                visitors.push(name.clone());
                let c = r.pick(&clients).unwrap().clone();
                format!(
                    "  {name}: {tag}\n    secret: {s}\n    reach:\n      {c}: any # reach {c}\n"
                )
            }
            _ => {
                port += 1;
                let c = r.pick(&clients).unwrap().clone();
                let t = 3000 + r.below(900);
                let vis = match r.pick(&visitors) {
                    Some(v) if r.chance(30) => format!("    visitors: [{v}]\n"),
                    _ => String::new(),
                };
                let web = if r.chance(25) {
                    let h = format!("Basic {}", Secret::generate().encode());
                    secrets.push(h.clone());
                    format!(
                        "    web:\n      hosts: [{name}.example.com]\n      request_headers:\n        Authorization: {h}\n"
                    )
                } else {
                    String::new()
                };
                if web.is_empty() && vis.is_empty() && r.chance(50) {
                    format!("  {name}: {{ client: {c}, bind: {port}, target: {t} }} {tag}\n")
                } else {
                    format!(
                        "  {name}: {tag}\n    client: {c} # client of {name}\n    bind: {port}\n    target: {t}\n{vis}{web}"
                    )
                }
            }
        };
        per_file[f].entry(sec).or_default().push(lines);
    }
    let w = &set.w;
    let mut main = format!(
        "# my server\nlisten: 7835 # tcp\nweb: {{ https: 443, http: 80, letsencrypt: {{ email: ops@example.com }} }}\n\nclients:\n  # the home box\n  home:\n    secret: {}\n{}\nvisitors:\n  ops:\n    secret: {}\n    admin: write\n{}\nservices:\n  web: {{ client: home, bind: 7840, target: 3000 }} # id0\n{}",
        w.home.encode(),
        per_file[0]
            .get("clients")
            .map(|v| v.concat())
            .unwrap_or_default(),
        w.ops.encode(),
        per_file[0]
            .get("visitors")
            .map(|v| v.concat())
            .unwrap_or_default(),
        per_file[0]
            .get("services")
            .map(|v| v.concat())
            .unwrap_or_default(),
    );
    if r.chance(30) {
        main.push_str("# trailing note\n");
    }
    let mut drop_ins = Vec::new();
    for (i, f) in per_file.iter().enumerate().skip(1) {
        let mut text = format!("# header f{i}\n");
        for sec in ["clients", "visitors", "services"] {
            if let Some(v) = f.get(sec) {
                text.push_str(&format!("{sec}:\n{}", v.concat()));
            }
        }
        drop_ins.push((format!("f{i}.yaml"), text));
    }
    Init {
        main,
        drop_ins,
        secrets,
    }
}

fn new_set(seed: u64) -> (Set, Init) {
    let set = Set::new(&[]);
    let init = init_for(seed, &set);
    for (name, text) in &init.drop_ins {
        crate::base::fs::write_private(&set.path(&format!("server.d/{name}")), text).unwrap();
    }
    crate::base::fs::write_private(&set.path(MAIN), &init.main).unwrap();
    (set, init)
}

type Item = (String, String);

fn file_items(text: &str) -> Option<BTreeMap<Item, Value>> {
    let v = crate::config::tree::parse(text).ok()?.to_json();
    let mut out = BTreeMap::new();
    if let Value::Object(top) = v {
        for (k, val) in top {
            if SECTIONS.contains(&k.as_str())
                && let Value::Object(items) = val
            {
                for (n, x) in items {
                    out.insert((k.clone(), n), x);
                }
            }
        }
    }
    Some(out)
}

fn disk(set: &Set) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    out.insert(MAIN.to_string(), set.read(MAIN).unwrap());
    if let Ok(rd) = std::fs::read_dir(set.path("server.d")) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.ends_with(".yaml") {
                out.insert(
                    format!("server.d/{n}"),
                    std::fs::read_to_string(e.path()).unwrap(),
                );
            }
        }
    }
    out
}

#[derive(Clone, Copy, Debug)]
enum Order {
    Ui,
    Api,
}

impl Order {
    fn of(self, ops: Vec<Op>) -> Vec<Op> {
        match self {
            Order::Ui => ui_order(ops),
            Order::Api => ops,
        }
    }
}

fn added_secret(op: &Op) -> Option<String> {
    match op {
        Op::Add { value, .. } => value
            .get("secret")
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    }
}

fn replay(seed: u64, changes: &[Vec<Op>]) -> Result<(), (usize, String)> {
    let (set, init) = new_set(seed);
    let s = set.session(true);
    check_start(&set).map_err(|e| (usize::MAX, e))?;
    let mut secrets = init.secrets.clone();
    secrets.extend(changes.iter().flatten().filter_map(added_secret));
    let mut stats = Stats::default();
    for (i, ops) in changes.iter().enumerate() {
        run_change(&set, &s, &secrets, ops, &mut stats).map_err(|e| (i, e))?;
    }
    Ok(())
}

fn check_start(set: &Set) -> Result<(), String> {
    let checked = plan::check_docs(&set.core, &set.core.docs().unwrap());
    if !checked.valid {
        let said: Vec<&String> = checked.diags.iter().map(|d| &d.message).collect();
        return Err(format!("init-invalid: {said:?}"));
    }
    Ok(())
}

fn kind(e: &str) -> &str {
    e.split(':').next().unwrap_or(e)
}

fn shrink(seed: u64, mut changes: Vec<Vec<Op>>, k: &str) -> Vec<Vec<Op>> {
    let fails = |c: &[Vec<Op>]| matches!(replay(seed, c), Err((_, e)) if kind(&e) == k);
    loop {
        let mut progress = false;
        let mut i = 0;
        while i < changes.len() {
            let mut c = changes.clone();
            c.remove(i);
            if fails(&c) {
                changes = c;
                progress = true;
            } else {
                i += 1;
            }
        }
        for ci in 0..changes.len() {
            let mut j = 0;
            while j < changes[ci].len() {
                let mut c = changes.clone();
                c[ci].remove(j);
                if !c[ci].is_empty() && fails(&c) {
                    changes = c;
                    progress = true;
                } else {
                    j += 1;
                }
            }
        }
        if !progress {
            return changes;
        }
    }
}

type Failure = (u64, Vec<Vec<Op>>, String);

fn run_seed(seed: u64, order: Order, stats: &mut Stats) -> Option<(String, Failure)> {
    let (set, init) = new_set(seed);
    if let Err(e) = check_start(&set) {
        panic!("seed {seed}: {e}\n{}\n{:?}", init.main, init.drop_ins);
    }
    let s = set.session(true);
    let mut r = Rng(seed);
    let mut newfile = 0;
    let mut changes: Vec<Vec<Op>> = Vec::new();
    let mut secrets = init.secrets.clone();
    for _ in 0..(1 + r.below(4)) {
        let ops = order.of(gen_change(&mut r, &disk(&set), &mut newfile));
        secrets.extend(ops.iter().filter_map(added_secret));
        changes.push(ops.clone());
        if let Err(e) = run_change(&set, &s, &secrets, &ops, stats) {
            return Some((kind(&e).to_string(), (seed, changes, e)));
        }
    }
    None
}

fn run_range(order: Order, lo: u64, hi: u64) {
    let mut stats = Stats::default();
    let mut failures: BTreeMap<String, Failure> = BTreeMap::new();
    let size = |c: &[Vec<Op>]| c.iter().map(Vec::len).sum::<usize>();
    for seed in lo..hi {
        if let Some((k, failed)) = run_seed(seed, order, &mut stats)
            && failures
                .get(&k)
                .is_none_or(|f| size(&f.1) > size(&failed.1))
        {
            failures.insert(k, failed);
        }
    }
    let mut report = format!("{order:?} order, seeds {lo}..{hi}: {}\n", stats.said());
    for (k, (seed, changes, e)) in &failures {
        let small = shrink(*seed, changes.clone(), k);
        let again = replay(*seed, &small);
        let (_, init) = new_set(*seed);
        report.push_str(&format!(
            "FAIL {k} seed {seed}\nfirst: {e}\nshrunk ops: {small:#?}\nshrunk result: {again:?}\nmain:\n{}\ndrop-ins: {:#?}\n",
            init.main, init.drop_ins
        ));
    }
    assert!(failures.is_empty(), "{report}");
    assert!(stats.applied() > 0, "{report}");
}

fn bounds() -> (u64, u64) {
    let read = |name: &str, default: u64| {
        std::env::var(name)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(default)
    };
    let start = read("PROBE_SEED", 0);
    (start, start + read("PROBE_CASES", 80))
}

#[test]
fn random_changes_sent_in_the_ui_order_keep_every_invariant() {
    let (lo, hi) = bounds();
    run_range(Order::Ui, lo, hi);
}

#[test]
fn random_changes_sent_in_any_api_order_keep_every_invariant() {
    let (lo, hi) = bounds();
    run_range(Order::Api, lo, hi);
}
