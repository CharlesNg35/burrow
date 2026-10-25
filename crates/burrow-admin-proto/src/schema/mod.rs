mod relation;
mod sem;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::Path;

pub use relation::{Cond, Relation};
pub use sem::{
    DurationC, DurationUnit, EnumC, EnumOption, HostNameC, IntegerC, IpC, IpPortC, ListC, MapC,
    NameC, ObjectC, OptionalC, PathC, PortRangeC, RefC, RefListC, RefMapC, SecretRefC, SemType,
    SizeC, TargetC, TextC, UnionC, UnknownType, UploadC,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Schema {
    pub schema_version: u32,
    #[serde(default)]
    pub hash: String,
    pub root: Node,
    #[serde(default)]
    pub entities: Vec<EntityInfo>,
}

impl Schema {
    pub fn locked_paths(&self) -> Vec<Path> {
        self.locks().into_iter().map(|(p, _)| p).collect()
    }

    pub fn locks(&self) -> Vec<(Path, Vec<usize>)> {
        fn walk(
            n: &Node,
            at: &mut Path,
            lists: &mut Vec<usize>,
            out: &mut Vec<(Path, Vec<usize>)>,
        ) {
            if n.locked {
                match out.iter_mut().find(|(p, _)| p == at) {
                    Some((_, l)) => l.retain(|k| lists.contains(k)),
                    None => out.push((at.clone(), lists.clone())),
                }
                return;
            }
            match &n.sem {
                SemType::Object(o) => {
                    for f in &o.fields {
                        at.push(f.key.clone());
                        walk(&f.node, at, lists, out);
                        at.pop();
                    }
                }
                SemType::Optional(o) => walk(&o.inner, at, lists, out),
                SemType::Union(u) => {
                    for b in &u.branches {
                        walk(b, at, lists, out);
                    }
                }
                SemType::Map(MapC { value, .. }) | SemType::RefMap(RefMapC { value, .. }) => {
                    at.push(ANY.into());
                    walk(value, at, lists, out);
                    at.pop();
                }
                SemType::List(l) => {
                    lists.push(at.len());
                    at.push(ANY.into());
                    walk(&l.item, at, lists, out);
                    at.pop();
                    lists.pop();
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        walk(&self.root, &mut Vec::new(), &mut Vec::new(), &mut out);
        out
    }

    pub fn reduce_only_paths(&self) -> Vec<Path> {
        fn walk(n: &Node, at: &mut Path, out: &mut Vec<Path>) {
            if n.reduce_only {
                out.push(at.clone());
            }
            let fields: Vec<(&str, &Node)> = match &n.sem {
                SemType::Object(o) => o.fields.iter().map(|f| (f.key.as_str(), &f.node)).collect(),
                SemType::Optional(o) => return walk(&o.inner, at, out),
                SemType::RefMap(m) => vec![("*", &*m.value)],
                SemType::Map(m) => vec![("*", &*m.value)],
                _ => return,
            };
            for (k, f) in fields {
                at.push(k.to_string());
                walk(f, at, out);
                at.pop();
            }
        }
        let mut out = Vec::new();
        walk(&self.root, &mut Vec::new(), &mut out);
        out
    }

    pub fn locked_at(&self, path: &[String]) -> Option<Path> {
        self.locked_paths()
            .into_iter()
            .find(|l| under_lock(l, path))
    }
}

pub const ANY: &str = "*";

pub fn under_lock(lock: &[String], path: &[String]) -> bool {
    lock.len() <= path.len() && lock.iter().zip(path).all(|(l, p)| l == ANY || l == p)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityInfo {
    pub entity: Entity,
    pub path: Path,
    pub label: String,
    #[serde(default)]
    pub plural: String,
    #[serde(default)]
    pub facets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Node {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub help: String,
    pub sem: SemType,
    #[serde(default, skip_serializing_if = "is_false")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "Editable::is_yes")]
    pub editable: Editable,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applies: Option<Applies>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub advanced: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub danger_when_set: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub locked: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub reduce_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wide: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub none_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub empty_label: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<Relation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<Group>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity: Option<Entity>,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub grammar: u8,
}

fn one() -> u8 {
    1
}

fn is_one(v: &u8) -> bool {
    *v == 1
}

fn is_false(v: &bool) -> bool {
    !v
}

impl Node {
    pub fn new(sem: SemType) -> Self {
        Node {
            sem,
            grammar: 1,
            ..Node::default()
        }
    }

    pub fn field(&self, key: &str) -> Option<&Field> {
        match &self.sem {
            SemType::Object(o) => o.fields.iter().find(|f| f.key == key),
            _ => None,
        }
    }

    pub fn at(&self, path: &[String]) -> Option<&Node> {
        let Some((first, rest)) = path.split_first() else {
            return Some(self);
        };
        let next = match &self.sem {
            SemType::Object(o) => o.fields.iter().find(|f| &f.key == first).map(|f| &f.node),
            SemType::Map(m) => Some(&*m.value),
            SemType::RefMap(m) => Some(&*m.value),
            SemType::List(l) => first.parse::<usize>().ok().map(|_| &*l.item),
            SemType::Optional(o) => return o.inner.at(path),
            SemType::Union(u) => return u.branches.iter().find_map(|b| b.at(path)),
            _ => None,
        }?;
        next.at(rest)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Field {
    pub key: String,
    pub node: Node,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub key: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sub: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum Editable {
    #[default]
    Yes,
    No {
        reason: String,
    },
}

impl Editable {
    pub fn is_yes(&self) -> bool {
        matches!(self, Editable::Yes)
    }
}

impl Serialize for Editable {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Editable::Yes => s.serialize_bool(true),
            Editable::No { reason } => serde_json::json!({ "no": reason }).serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for Editable {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        Ok(match v {
            Value::Bool(false) => Editable::No {
                reason: String::new(),
            },
            Value::Object(m) => Editable::No {
                reason: m
                    .get("no")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            },
            _ => Editable::Yes,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Applies {
    Live,
    Reload,
    Restart,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Entity {
    Client,
    Visitor,
    Service,
    Unknown(String),
}

impl Entity {
    pub fn as_str(&self) -> &str {
        match self {
            Entity::Client => "client",
            Entity::Visitor => "visitor",
            Entity::Service => "service",
            Entity::Unknown(s) => s,
        }
    }

    pub fn plural(&self) -> &'static str {
        match self {
            Entity::Client => "Clients",
            Entity::Visitor => "Visitors",
            Entity::Service | Entity::Unknown(_) => "Services",
        }
    }

    pub fn parse(s: &str) -> Entity {
        match s {
            "client" => Entity::Client,
            "visitor" => Entity::Visitor,
            "service" => Entity::Service,
            other => Entity::Unknown(other.to_string()),
        }
    }

    pub fn section(&self) -> &str {
        match self {
            Entity::Client => "clients",
            Entity::Visitor => "visitors",
            Entity::Service => "services",
            Entity::Unknown(s) => s,
        }
    }
}

impl Serialize for Entity {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Entity {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Entity::parse(&s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn known_types_round_trip() {
        let n = Node {
            label: "Public address".into(),
            sem: SemType::Optional(OptionalC {
                inner: Box::new(Node::new(SemType::IpPort(IpPortC {
                    ranges: true,
                    max_range: Some(1000),
                    default_ip: Some("0.0.0.0".into()),
                }))),
            }),
            relations: vec![
                Relation::VisibleIf(Cond::LenGe("clients".into(), 2)),
                Relation::EnabledIf {
                    cond: Cond::Eq("protocol".into(), json!("tcp")),
                    reason: "UDP services do not support this".into(),
                },
            ],
            ..Node::default()
        };
        let v = serde_json::to_value(&n).unwrap();
        assert_eq!(v["sem"]["t"], "optional");
        assert_eq!(v["sem"]["inner"]["sem"]["t"], "ip_port");
        let back: Node = serde_json::from_value(v).unwrap();
        assert_eq!(back, n);
        assert_eq!(
            serde_json::to_value(SemType::Bool).unwrap(),
            json!({"t": "bool"})
        );
    }

    #[test]
    fn unknown_types_keep_their_fallback_and_raw_value() {
        let raw =
            json!({"t": "geo_fence", "fallback": [{"t": "cidr"}, {"t": "text"}], "radius": 5});
        let s: SemType = serde_json::from_value(raw.clone()).unwrap();
        let SemType::Unknown(u) = &s else {
            panic!("{s:?}")
        };
        assert_eq!(u.t, "geo_fence");
        assert_eq!(
            u.fallback,
            vec![SemType::Cidr, SemType::Text(TextC::default())]
        );
        assert_eq!(serde_json::to_value(&s).unwrap(), raw);
        let broken: SemType = serde_json::from_value(json!({"t": "ref"})).unwrap();
        assert!(matches!(broken, SemType::Unknown(_)));
        assert!(serde_json::from_value::<SemType>(json!({"x": 1})).is_err());
    }

    #[test]
    fn a_host_name_tells_older_uis_to_show_a_text_field() {
        let s = SemType::HostName(HostNameC::default());
        let sent = serde_json::to_value(&s).unwrap();
        assert_eq!(sent, json!({"t": "host_name", "fallback": [{"t": "text"}]}));
        assert_eq!(serde_json::from_value::<SemType>(sent).unwrap(), s);
        let mut older = serde_json::to_value(&s).unwrap();
        older["t"] = json!("host_name_v0");
        let SemType::Unknown(u) = serde_json::from_value::<SemType>(older).unwrap() else {
            panic!("an unknown tag stays unknown")
        };
        assert_eq!(u.fallback, vec![SemType::Text(TextC::default())]);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let n: Node = serde_json::from_value(
            json!({"sem": {"t": "integer", "min": 1, "future": true}, "later": 3}),
        )
        .unwrap();
        assert!(matches!(
            n.sem,
            SemType::Integer(IntegerC { min: Some(1), .. })
        ));
        assert_eq!(n.grammar, 1);
    }

    #[test]
    fn conditions_and_relations_survive_unknown_operators() {
        let c = Cond::from_json(&json!({"all": [{"set": "admin"}, {"geo": 1}]}));
        assert_eq!(
            c,
            Cond::All(vec![
                Cond::Set("admin".into()),
                Cond::Unknown(json!({"geo": 1}))
            ])
        );
        assert_eq!(Cond::from_json(&c.to_json()), c);
        let r: Relation = serde_json::from_value(json!({"r": "teleport", "x": 1})).unwrap();
        assert_eq!(r, Relation::Unknown(json!({"r": "teleport", "x": 1})));
        let e: Entity = serde_json::from_value(json!("router")).unwrap();
        assert_eq!(e, Entity::Unknown("router".into()));
        let ed: Editable = serde_json::from_value(json!({"no": "managed elsewhere"})).unwrap();
        assert_eq!(
            ed,
            Editable::No {
                reason: "managed elsewhere".into()
            }
        );
    }
}
