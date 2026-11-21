#![cfg(test)]

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use super::super::drop_ins::probe::Rng;
use crate::config::Format;
use crate::config::emit::render;

pub(super) const CASES: usize = 160;
const MAX_DEPTH: usize = 12;

pub(super) struct Generated {
    pub(super) text: String,
    pub(super) with_env: String,
    pub(super) env: BTreeMap<String, String>,
    value: Value,
}

struct Gen<'a> {
    root: &'a Value,
    rng: Rng,
}

fn types(s: &Value) -> Vec<&str> {
    match s.get("type") {
        Some(Value::String(t)) => vec![t.as_str()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

fn sample(key: &str) -> Value {
    let s = match key {
        "email" => "ops@example.com",
        "hosts" | "domains" | "host" | "redirect_from" => "app.example.com",
        "listen" | "bind" | "http" | "https" => "127.0.0.1:7900",
        "target" => "127.0.0.1:3000",
        "client" | "name" => "home",
        "path" | "app_path" => "/app",
        _ => "x1",
    };
    json!(s)
}

fn resolve<'a>(root: &'a Value, mut s: &'a Value) -> &'a Value {
    for _ in 0..8 {
        match s.get("$ref").and_then(Value::as_str) {
            Some(r) => s = &root["$defs"][r.trim_start_matches("#/$defs/")],
            None => break,
        }
    }
    s
}

impl<'a> Gen<'a> {
    fn value(&mut self, s: &'a Value, key: &str, depth: usize) -> Value {
        let s = resolve(self.root, s);
        if depth > MAX_DEPTH {
            return Value::Null;
        }
        if let Some(branches) = s.get("anyOf").and_then(Value::as_array) {
            let real: Vec<&Value> = branches.iter().filter(|b| types(b) != ["null"]).collect();
            let pick = match self.rng.chance(10) || real.is_empty() {
                true => &branches[self.rng.below(branches.len())],
                false => real[self.rng.below(real.len())],
            };
            return self.value(pick, key, depth + 1);
        }
        if let Some(options) = s.get("enum").and_then(Value::as_array) {
            return options[self.rng.below(options.len())].clone();
        }
        let t = types(s);
        let real: Vec<&str> = t.iter().copied().filter(|t| *t != "null").collect();
        let pick = match real.is_empty() {
            true => "null",
            false => real[self.rng.below(real.len())],
        };
        match pick {
            "object" => self.object(s, key, depth),
            "array" => match s.get("items") {
                Some(item) => {
                    let n = 1 + self.rng.below(2);
                    Value::Array((0..n).map(|_| self.value(item, key, depth + 1)).collect())
                }
                None => json!([]),
            },
            "integer" => {
                let min = s.get("minimum").and_then(Value::as_i64).unwrap_or(0);
                let max = s
                    .get("maximum")
                    .and_then(Value::as_i64)
                    .unwrap_or(min + 500)
                    .min(min + 500);
                json!(min + self.rng.below((max - min + 1) as usize) as i64)
            }
            "number" => json!(1.5),
            "boolean" => json!(self.rng.chance(50)),
            "string" => sample(key),
            _ => Value::Null,
        }
    }

    fn object(&mut self, s: &'a Value, key: &str, depth: usize) -> Value {
        let mut out = Map::new();
        let required: Vec<&str> = s["required"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if let Some(props) = s.get("properties").and_then(Value::as_object) {
            for (k, p) in props {
                if required.contains(&k.as_str()) || self.rng.chance(45) {
                    let v = self.value(p, k, depth + 1);
                    out.insert(k.clone(), v);
                }
            }
        }
        if let Some(ap) = s.get("additionalProperties").filter(|a| a.is_object()) {
            for i in 0..1 + self.rng.below(2) {
                let name = match key {
                    "request_headers" | "response_headers" if i == 0 => "Authorization".to_string(),
                    _ => format!("n{i}"),
                };
                let v = self.value(ap, key, depth + 1);
                out.insert(name, v);
            }
        }
        Value::Object(out)
    }
}

fn with_env(v: &Value, rng: &mut Rng, env: &mut BTreeMap<String, String>) -> Value {
    let var = |text: String, env: &mut BTreeMap<String, String>| {
        let name = format!("V{}", env.len());
        env.insert(name.clone(), text);
        json!(format!("${{{name}}}"))
    };
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, x)| (k.clone(), with_env(x, rng, env)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(|x| with_env(x, rng, env)).collect()),
        Value::Number(n) if rng.chance(60) => var(n.to_string(), env),
        Value::Bool(b) if rng.chance(60) => var(b.to_string(), env),
        Value::String(s) if rng.chance(40) => var(s.clone(), env),
        other => other.clone(),
    }
}

pub(super) fn config(source: &Value, seed: usize) -> Generated {
    let mut g = Gen {
        root: source,
        rng: Rng((seed as u64).wrapping_mul(0x9E37_79B9) ^ 0xED17),
    };
    let value = g.value(source, "", 0);
    let mut env = BTreeMap::new();
    let swapped = with_env(&value, &mut g.rng, &mut env);
    Generated {
        text: render(&value, Format::Yaml),
        with_env: render(&swapped, Format::Yaml),
        env,
        value,
    }
}

fn places(v: &Value, at: &mut Vec<String>, out: &mut Vec<(Vec<String>, bool)>) {
    match v {
        Value::Object(m) => {
            out.push((at.clone(), true));
            for (k, x) in m {
                at.push(k.clone());
                places(x, at, out);
                at.pop();
            }
        }
        Value::Array(a) => a.iter().enumerate().for_each(|(i, x)| {
            at.push(i.to_string());
            places(x, at, out);
            at.pop();
        }),
        Value::Number(_) | Value::Bool(_) => out.push((at.clone(), false)),
        _ => {}
    }
}

fn at_mut<'a>(v: &'a mut Value, path: &[String]) -> Option<&'a mut Value> {
    path.iter().try_fold(v, |cur, k| match cur {
        Value::Object(m) => m.get_mut(k),
        Value::Array(a) => a.get_mut(k.parse::<usize>().ok()?),
        _ => None,
    })
}

pub(super) fn refusal(g: &Generated, seed: usize) -> Option<(String, String, Vec<String>)> {
    let mut rng = Rng(seed as u64 ^ 0xBAD);
    let mut found = Vec::new();
    places(&g.value, &mut Vec::new(), &mut found);
    let (path, object) = found.get(rng.below(found.len()))?.clone();
    let mut v = g.value.clone();
    let target = at_mut(&mut v, &path)?;
    let (what, path) = if object {
        target
            .as_object_mut()?
            .insert("zz_unknown".into(), json!(1));
        let mut p = path.clone();
        p.push("zz_unknown".into());
        ("an unknown key", p)
    } else {
        *target = json!("not-a-value");
        ("a wrong type", path)
    };
    Some((what.into(), render(&v, Format::Yaml), path))
}
