use std::fmt;

use burrow_admin_proto::serde_json::Value;
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::admin_schema::AdminSchema;
use crate::model::{ServiceEntry, Target};

pub use burrow_admin_proto::schema::WEIGHT_MAX;

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "json-schema", serde(untagged))]
pub enum ClientRef {
    Name(String),
    Entry(ServiceClient),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[admin(label = "Client entry")]
pub struct ServiceClient {
    #[schemars(description = "Name of the client")]
    #[admin(ty = ref_(client, reserved = "self"), label = "Client")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Where this client connects instead of the main target, for example 10.0.0.12:3000; a range must have as many ports as the main target"
    )]
    #[admin(
        help = "Where this client connects instead of the main target, for example 10.0.0.12:3000",
        none_label = "The main target"
    )]
    pub target: Option<Target>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Share of new connections with mode: balance, or of visitor addresses with mode: ip_hash, from 1 to 1000 (default 1); a client with weight 3 gets three times the share of one with weight 1"
    )]
    #[schemars(range(min = 1, max = 1000))]
    #[admin(ty = integer(unit = "shares"), none_label = "1")]
    pub weight: Option<i64>,
}

impl ClientRef {
    pub fn name(&self) -> &str {
        match self {
            ClientRef::Name(n) => n,
            ClientRef::Entry(e) => &e.name,
        }
    }

    pub fn given_weight(&self) -> Option<i64> {
        match self {
            ClientRef::Name(_) => None,
            ClientRef::Entry(e) => e.weight,
        }
    }

    pub fn weight(&self) -> u32 {
        self.given_weight()
            .filter(|w| (1..=WEIGHT_MAX).contains(w))
            .map_or(1, |w| w as u32)
    }

    pub fn target(&self) -> Option<&Target> {
        match self {
            ClientRef::Name(_) => None,
            ClientRef::Entry(e) => e.target.as_ref(),
        }
    }
}

impl Serialize for ClientRef {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            ClientRef::Entry(e) if e.target.is_some() || e.weight.is_some() => e.serialize(s),
            other => s.serialize_str(other.name()),
        }
    }
}

impl From<&str> for ClientRef {
    fn from(name: &str) -> Self {
        ClientRef::Name(name.to_string())
    }
}

impl From<String> for ClientRef {
    fn from(name: String) -> Self {
        ClientRef::Name(name)
    }
}

impl PartialEq<str> for ClientRef {
    fn eq(&self, other: &str) -> bool {
        self.name() == other
    }
}

impl PartialEq<&str> for ClientRef {
    fn eq(&self, other: &&str) -> bool {
        self.name() == *other
    }
}

impl PartialEq<String> for ClientRef {
    fn eq(&self, other: &String) -> bool {
        self.name() == other
    }
}

impl fmt::Display for ClientRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl<'de> Deserialize<'de> for ClientRef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = ClientRef;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a client name, or {name, target, weight}")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<ClientRef, E> {
                Ok(ClientRef::Name(v.to_string()))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<ClientRef, E> {
                Ok(ClientRef::Name(v))
            }
            fn visit_map<M: MapAccess<'de>>(self, m: M) -> Result<ClientRef, M::Error> {
                ServiceClient::deserialize(de::value::MapAccessDeserializer::new(m))
                    .map(ClientRef::Entry)
            }
        }
        d.deserialize_any(V)
    }
}

pub fn ref_name(v: &Value) -> Option<&str> {
    v.as_str().or_else(|| ServiceClient::NAME.str(v))
}

impl ServiceEntry {
    pub fn listed_names(v: Option<&Value>) -> Vec<String> {
        match v {
            Some(Value::Array(a)) => a.iter().filter_map(ref_name).map(str::to_string).collect(),
            Some(Value::String(s)) => vec![s.clone()],
            _ => Vec::new(),
        }
    }

    pub fn with_client(svc: &Value, name: &str) -> Vec<Value> {
        let mut entries: Vec<Value> = match (
            ServiceEntry::CLIENT.get(svc),
            ServiceEntry::CLIENTS.get(svc),
        ) {
            (Some(c @ Value::String(_)), _) => vec![c.clone()],
            (_, Some(Value::Array(a))) => a.clone(),
            _ => Vec::new(),
        };
        entries.push(Value::String(name.to_string()));
        entries
    }

    pub fn client_names(svc: &Value) -> Vec<String> {
        let mut names = Self::listed_names(ServiceEntry::CLIENT.get(svc));
        for n in Self::listed_names(ServiceEntry::CLIENTS.get(svc)) {
            if !names.contains(&n) {
                names.push(n);
            }
        }
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::serde_json::{self, json};

    #[test]
    fn an_entry_is_a_name_or_a_name_with_a_weight() {
        let list: Vec<ClientRef> =
            serde_json::from_value(json!(["a", {"name": "b", "weight": 3}, {"name": "c"}]))
                .unwrap();
        assert_eq!(list[0], ClientRef::from("a"));
        assert_eq!((list[1].name(), list[1].weight()), ("b", 3));
        assert_eq!((list[2].name(), list[2].given_weight()), ("c", None));
        assert_eq!(list[2].weight(), 1);
        assert_eq!(
            serde_json::to_value(&list).unwrap(),
            json!(["a", {"name": "b", "weight": 3}, "c"])
        );
    }

    #[test]
    fn a_misspelt_key_is_named() {
        let e = serde_json::from_value::<ClientRef>(json!({"name": "a", "wieght": 3}))
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown field `wieght`"), "{e}");
        assert!(e.contains("`weight`"), "{e}");
        let e = serde_json::from_value::<ClientRef>(json!(7))
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("a client name, or {name, target, weight}"),
            "{e}"
        );
    }

    #[test]
    fn names_come_from_client_and_both_entry_forms() {
        let svc = json!({"clients": ["a", {"name": "b", "weight": 2}, 5]});
        assert_eq!(ServiceEntry::client_names(&svc), vec!["a", "b"]);
        assert_eq!(
            ServiceEntry::client_names(&json!({"client": "x"})),
            vec!["x"]
        );
        assert_eq!(ref_name(&json!({"weight": 2})), None);
    }

    #[test]
    fn a_new_client_is_added_after_the_entries_and_keeps_them() {
        let svc = json!({"clients": ["a", {"name": "b", "target": 8080, "weight": 2}]});
        assert_eq!(
            ServiceEntry::with_client(&svc, "c"),
            vec![
                json!("a"),
                json!({"name": "b", "target": 8080, "weight": 2}),
                json!("c")
            ]
        );
        assert_eq!(
            ServiceEntry::with_client(&json!({"client": "a"}), "c"),
            vec![json!("a"), json!("c")]
        );
    }

    #[test]
    fn an_entry_with_its_own_target_keeps_it_and_a_bare_entry_is_written_as_a_name() {
        let list: Vec<ClientRef> = serde_json::from_value(json!([
            {"name": "gw", "target": "10.0.0.12:3000"},
            {"name": "gw", "target": 3001, "weight": 2}
        ]))
        .unwrap();
        assert_eq!(
            list[0].target(),
            Some(&Target::Addr("10.0.0.12:3000".into()))
        );
        assert_eq!(list[1].target(), Some(&Target::Port(3001)));
        assert_eq!(
            serde_json::to_value(ClientRef::Entry(ServiceClient {
                name: "a".into(),
                target: None,
                weight: None
            }))
            .unwrap(),
            json!("a")
        );
    }
}
