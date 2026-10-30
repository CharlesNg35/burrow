use std::fmt;

use burrow_admin_proto::serde_json::Value;
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::admin_schema::AdminSchema;
use crate::model::ServiceEntry;

pub const WEIGHT_MAX: i64 = 1000;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum ClientRef {
    Name(String),
    Weighted(WeightedClient),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[admin(label = "Client with a weight")]
pub struct WeightedClient {
    #[schemars(description = "Name of the client")]
    #[admin(ty = ref_(client, reserved = "self"), label = "Client")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Share of new connections with mode: balance, from 1 to 1000 (default 1); a client with weight 3 gets three times the new connections of one with weight 1"
    )]
    #[schemars(range(min = 1, max = 1000))]
    #[admin(ty = integer(unit = "shares"), none_label = "1")]
    pub weight: Option<i64>,
}

impl ClientRef {
    pub fn name(&self) -> &str {
        match self {
            ClientRef::Name(n) => n,
            ClientRef::Weighted(w) => &w.name,
        }
    }

    pub fn given_weight(&self) -> Option<i64> {
        match self {
            ClientRef::Name(_) => None,
            ClientRef::Weighted(w) => w.weight,
        }
    }

    pub fn weight(&self) -> u32 {
        self.given_weight()
            .filter(|w| (1..=WEIGHT_MAX).contains(w))
            .map_or(1, |w| w as u32)
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
                f.write_str("a client name, or {name, weight}")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<ClientRef, E> {
                Ok(ClientRef::Name(v.to_string()))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<ClientRef, E> {
                Ok(ClientRef::Name(v))
            }
            fn visit_map<M: MapAccess<'de>>(self, m: M) -> Result<ClientRef, M::Error> {
                WeightedClient::deserialize(de::value::MapAccessDeserializer::new(m))
                    .map(ClientRef::Weighted)
            }
        }
        d.deserialize_any(V)
    }
}

pub fn ref_name(v: &Value) -> Option<&str> {
    v.as_str().or_else(|| WeightedClient::NAME.str(v))
}

impl ServiceEntry {
    pub fn listed_names(v: Option<&Value>) -> Vec<String> {
        match v {
            Some(Value::Array(a)) => a.iter().filter_map(ref_name).map(str::to_string).collect(),
            Some(Value::String(s)) => vec![s.clone()],
            _ => Vec::new(),
        }
    }

    pub fn keep_weights(old: Option<&Value>, names: &[String]) -> Vec<Value> {
        let old: &[Value] = old.and_then(Value::as_array).map_or(&[], Vec::as_slice);
        names
            .iter()
            .map(|n| {
                old.iter()
                    .find(|v| v.is_object() && ref_name(v) == Some(n))
                    .cloned()
                    .unwrap_or_else(|| Value::String(n.clone()))
            })
            .collect()
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
            json!(["a", {"name": "b", "weight": 3}, {"name": "c"}])
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
        assert!(e.contains("a client name, or {name, weight}"), "{e}");
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
    fn a_new_list_of_names_keeps_the_weights_it_had() {
        let old = json!(["a", {"name": "b", "weight": 2}]);
        let names = ["b".to_string(), "c".to_string()];
        assert_eq!(
            ServiceEntry::keep_weights(Some(&old), &names),
            vec![json!({"name": "b", "weight": 2}), json!("c")]
        );
        assert_eq!(
            ServiceEntry::keep_weights(None, &names[1..]),
            vec![json!("c")]
        );
    }
}
