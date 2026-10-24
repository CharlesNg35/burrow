use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use super::Entity;

#[derive(Debug, Clone, PartialEq)]
pub enum Cond {
    Eq(String, Value),
    Ne(String, Value),
    LenGe(String, u64),
    Set(String),
    All(Vec<Cond>),
    Any(Vec<Cond>),
    Not(Box<Cond>),
    Unknown(Value),
}

impl Serialize for Cond {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.to_json().serialize(s)
    }
}

impl<'de> Deserialize<'de> for Cond {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Cond::from_json(&Value::deserialize(d)?))
    }
}

impl Cond {
    pub fn to_json(&self) -> Value {
        use serde_json::json;
        match self {
            Cond::Eq(p, v) => json!({ "eq": [p, v] }),
            Cond::Ne(p, v) => json!({ "ne": [p, v] }),
            Cond::LenGe(p, n) => json!({ "len_ge": [p, n] }),
            Cond::Set(p) => json!({ "set": p }),
            Cond::All(c) => json!({ "all": c.iter().map(Cond::to_json).collect::<Vec<_>>() }),
            Cond::Any(c) => json!({ "any": c.iter().map(Cond::to_json).collect::<Vec<_>>() }),
            Cond::Not(c) => json!({ "not": c.to_json() }),
            Cond::Unknown(v) => v.clone(),
        }
    }

    pub fn from_json(v: &Value) -> Cond {
        let unknown = || Cond::Unknown(v.clone());
        let Some(obj) = v.as_object() else {
            return unknown();
        };
        if obj.len() != 1 {
            return unknown();
        }
        let (k, arg) = obj.iter().next().expect("one entry");
        let pair = || {
            let a = arg.as_array()?;
            match a.as_slice() {
                [p, x] => Some((p.as_str()?.to_string(), x.clone())),
                _ => None,
            }
        };
        let list = || {
            arg.as_array()
                .map(|a| a.iter().map(Cond::from_json).collect::<Vec<_>>())
        };
        match k.as_str() {
            "eq" => pair().map(|(p, x)| Cond::Eq(p, x)),
            "ne" => pair().map(|(p, x)| Cond::Ne(p, x)),
            "len_ge" => pair().and_then(|(p, x)| x.as_u64().map(|n| Cond::LenGe(p, n))),
            "set" => arg.as_str().map(|p| Cond::Set(p.to_string())),
            "all" => list().map(Cond::All),
            "any" => list().map(Cond::Any),
            "not" => Some(Cond::Not(Box::new(Cond::from_json(arg)))),
            _ => None,
        }
        .unwrap_or_else(unknown)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Relation {
    VisibleIf(Cond),
    EnabledIf {
        cond: Cond,
        reason: String,
    },
    OneOf {
        group: String,
    },
    OptionsFilter(Cond),
    Inverse {
        entity: Entity,
        field: String,
        label: String,
    },
    DefaultFrom(String),
    Cross(Vec<String>),
    SameLen(String),
    WarnIf {
        cond: Cond,
        message: String,
    },
    Unknown(Value),
}

impl Serialize for Relation {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde_json::json;
        let v = match self {
            Relation::VisibleIf(c) => json!({ "r": "visible_if", "cond": c }),
            Relation::EnabledIf { cond, reason } => {
                json!({ "r": "enabled_if", "cond": cond, "reason": reason })
            }
            Relation::OneOf { group } => json!({ "r": "one_of", "group": group }),
            Relation::OptionsFilter(c) => json!({ "r": "options_filter", "cond": c }),
            Relation::Inverse {
                entity,
                field,
                label,
            } => json!({ "r": "inverse", "entity": entity, "field": field, "label": label }),
            Relation::DefaultFrom(p) => json!({ "r": "default_from", "path": p }),
            Relation::Cross(p) => json!({ "r": "cross", "paths": p }),
            Relation::SameLen(p) => json!({ "r": "same_len", "path": p }),
            Relation::WarnIf { cond, message } => {
                json!({ "r": "warn_if", "cond": cond, "message": message })
            }
            Relation::Unknown(v) => v.clone(),
        };
        v.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Relation {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        let cond = || v.get("cond").map(Cond::from_json);
        let r = match v.get("r").and_then(Value::as_str) {
            Some("visible_if") => cond().map(Relation::VisibleIf),
            Some("enabled_if") => cond().map(|cond| Relation::EnabledIf {
                cond,
                reason: s("reason").unwrap_or_default(),
            }),
            Some("one_of") => s("group").map(|group| Relation::OneOf { group }),
            Some("options_filter") => cond().map(Relation::OptionsFilter),
            Some("inverse") => match (v.get("entity"), s("field")) {
                (Some(e), Some(field)) => e.as_str().map(|e| Relation::Inverse {
                    entity: Entity::parse(e),
                    field,
                    label: s("label").unwrap_or_default(),
                }),
                _ => None,
            },
            Some("default_from") => s("path").map(Relation::DefaultFrom),
            Some("cross") => v.get("paths").and_then(Value::as_array).map(|a| {
                Relation::Cross(
                    a.iter()
                        .filter_map(|p| p.as_str().map(str::to_string))
                        .collect(),
                )
            }),
            Some("same_len") => s("path").map(Relation::SameLen),
            Some("warn_if") => cond().map(|cond| Relation::WarnIf {
                cond,
                message: s("message").unwrap_or_default(),
            }),
            _ => None,
        };
        Ok(r.unwrap_or(Relation::Unknown(v)))
    }
}
