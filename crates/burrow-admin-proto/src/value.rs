use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Path;

pub const SECRET_KEY: &str = "$secret";
pub const ENV_KEY: &str = "$env";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretSrc {
    Literal,
    Env(String),
}

impl Serialize for SecretSrc {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            SecretSrc::Literal => s.serialize_str("literal"),
            SecretSrc::Env(v) => json!({ "env": v }).serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for SecretSrc {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        Ok(match v.get("env").and_then(Value::as_str) {
            Some(name) => SecretSrc::Env(name.to_string()),
            None => SecretSrc::Literal,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretInfo {
    pub fp: Option<String>,
    pub src: SecretSrc,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unset: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvInfo {
    pub raw: String,
    #[serde(default)]
    pub vars: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved: Option<String>,
}

pub fn secret(info: &SecretInfo) -> Value {
    json!({ SECRET_KEY: info })
}

pub fn env(info: &EnvInfo) -> Value {
    json!({ ENV_KEY: info })
}

pub fn as_secret(v: &Value) -> Option<SecretInfo> {
    let o = v.as_object()?;
    if o.len() != 1 {
        return None;
    }
    serde_json::from_value(o.get(SECRET_KEY)?.clone()).ok()
}

pub fn as_env(v: &Value) -> Option<EnvInfo> {
    let o = v.as_object()?;
    if o.len() != 1 {
        return None;
    }
    serde_json::from_value(o.get(ENV_KEY)?.clone()).ok()
}

pub fn is_special(v: &Value) -> bool {
    as_secret(v).is_some() || as_env(v).is_some()
}

pub fn get<'a>(root: &'a Value, path: &[String]) -> Option<&'a Value> {
    path.iter().try_fold(root, |v, k| match v {
        Value::Object(m) => m.get(k),
        Value::Array(a) => k.parse::<usize>().ok().and_then(|i| a.get(i)),
        _ => None,
    })
}

pub fn get_mut<'a>(root: &'a mut Value, path: &[String]) -> Option<&'a mut Value> {
    path.iter().try_fold(root, |v, k| match v {
        Value::Object(m) => m.get_mut(k),
        Value::Array(a) => k.parse::<usize>().ok().and_then(move |i| a.get_mut(i)),
        _ => None,
    })
}

pub fn put(root: &mut Value, path: &[String], v: Option<Value>) -> bool {
    let Some((last, parents)) = path.split_last() else {
        return false;
    };
    let mut cur = root;
    for k in parents {
        if cur.is_null() {
            *cur = Value::Object(serde_json::Map::new());
        }
        let Some(m) = cur.as_object_mut() else {
            return false;
        };
        cur = m.entry(k.clone()).or_insert(Value::Null);
    }
    if cur.is_null() {
        *cur = Value::Object(serde_json::Map::new());
    }
    let Some(m) = cur.as_object_mut() else {
        return false;
    };
    match v {
        Some(v) => {
            m.insert(last.clone(), v);
        }
        None => {
            m.shift_remove(last);
        }
    }
    true
}

pub fn child(path: &[String], key: &str) -> Path {
    let mut p = path.to_vec();
    p.push(key.to_string());
    p
}

pub fn one_or_many(v: Option<&Value>) -> Vec<Value> {
    match v {
        Some(Value::Array(a)) => a.clone(),
        Some(Value::Null) | None => Vec::new(),
        Some(one) => vec![one.clone()],
    }
}

pub fn display(v: &Value) -> String {
    if let Some(s) = as_secret(v) {
        return match (s.fp, s.src) {
            (Some(fp), SecretSrc::Literal) => format!("secret {fp}"),
            (Some(fp), SecretSrc::Env(e)) => format!("${{{e}}} (secret {fp})"),
            (None, SecretSrc::Env(e)) => format!("${{{e}}} (unset)"),
            (None, SecretSrc::Literal) => "secret".into(),
        };
    }
    if let Some(e) = as_env(v) {
        return e.raw;
    }
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(display).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn special_values_round_trip() {
        let s = SecretInfo {
            fp: Some("ab12cd34".into()),
            src: SecretSrc::Env("HOME_SECRET".into()),
            unset: false,
        };
        let v = secret(&s);
        assert_eq!(
            v,
            json!({"$secret": {"fp": "ab12cd34", "src": {"env": "HOME_SECRET"}}})
        );
        assert_eq!(as_secret(&v), Some(s));
        let lit = json!({"$secret": {"fp": "ab12cd34", "src": "literal"}});
        assert_eq!(as_secret(&lit).unwrap().src, SecretSrc::Literal);
        let e = EnvInfo {
            raw: "${PORT}".into(),
            vars: vec!["PORT".into()],
            resolved: Some("7840".into()),
        };
        assert_eq!(as_env(&env(&e)), Some(e));
        assert!(as_env(&json!({"$env": {"raw": "x"}, "y": 1})).is_none());
        assert_eq!(display(&lit), "secret ab12cd34");
    }

    #[test]
    fn paths() {
        let mut v = json!({"a": {"b": [1, {"c": 2}]}});
        let p = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(get(&v, &p(&["a", "b", "1", "c"])), Some(&json!(2)));
        assert_eq!(get(&v, &p(&["a", "x"])), None);
        *get_mut(&mut v, &p(&["a", "b", "0"])).unwrap() = json!(5);
        assert_eq!(v["a"]["b"][0], 5);
    }
}
