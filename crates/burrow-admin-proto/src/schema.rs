use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::Path;

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
        fn walk(n: &Node, at: &mut Path, out: &mut Vec<Path>) {
            if n.locked {
                out.push(at.clone());
                return;
            }
            let fields = match &n.sem {
                SemType::Object(o) => &o.fields,
                SemType::Optional(o) => {
                    return walk(&o.inner, at, out);
                }
                _ => return,
            };
            for f in fields {
                at.push(f.key.clone());
                walk(&f.node, at, out);
                at.pop();
            }
        }
        let mut out = Vec::new();
        walk(&self.root, &mut Vec::new(), &mut out);
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
            .find(|l| path.starts_with(l) || (l.starts_with(path) && !path.is_empty()))
    }
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

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TextC {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_len: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct IntegerC {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct EnumC {
    pub options: Vec<EnumOption>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnumOption {
    pub value: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub help: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub danger: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IpC {
    #[serde(default = "yes")]
    pub v4: bool,
    #[serde(default = "yes")]
    pub v6: bool,
    #[serde(default = "yes")]
    pub unspecified_ok: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct IpPortC {
    #[serde(default)]
    pub ranges: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_range: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_ip: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PortRangeC {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TargetC {
    #[serde(default)]
    pub ranges: bool,
    #[serde(default)]
    pub unix: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurationUnit {
    Secs,
    Ms,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DurationC {
    pub unit: DurationUnit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SizeC {
    #[serde(default)]
    pub unit: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RefC {
    pub entity: Entity,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reserved: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RefListC {
    pub entity: Entity,
    #[serde(default)]
    pub ordered: bool,
    #[serde(default = "yes")]
    pub unique: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reserved: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RefMapC {
    pub entity: Entity,
    pub value: Box<Node>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reserved: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NameC {
    pub entity: Entity,
    pub max_len: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecretRefC {
    pub max: u32,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PathC {
    #[serde(default)]
    pub must_exist: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ObjectC {
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MapC {
    pub key: Box<Node>,
    pub value: Box<Node>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListC {
    pub item: Box<Node>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnionC {
    pub branches: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OptionalC {
    pub inner: Box<Node>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UnknownType {
    pub t: String,
    pub fallback: Vec<SemType>,
    pub raw: Value,
}

macro_rules! sem_types {
    ($($variant:ident $tag:literal $(($conf:ty))?),* $(,)?) => {
        #[derive(Debug, Clone, PartialEq)]
        pub enum SemType {
            $($variant $(($conf))?,)*
            Unknown(UnknownType),
        }

        #[derive(Serialize, Deserialize)]
        #[serde(tag = "t")]
        enum Known {
            $(#[serde(rename = $tag)] $variant $(($conf))?,)*
        }

        impl SemType {
            pub const KNOWN_TAGS: &'static [&'static str] = &[$($tag),*];

            pub fn tag(&self) -> &str {
                match self {
                    $(sem_types!(@pat $variant $(($conf))?) => $tag,)*
                    SemType::Unknown(u) => &u.t,
                }
            }

            fn known(&self) -> Option<Known> {
                match self.clone() {
                    $(sem_types!(@bind c $variant $(($conf))?) => Some(sem_types!(@make c Known $variant $(($conf))?)),)*
                    SemType::Unknown(_) => None,
                }
            }

            fn from_known(k: Known) -> SemType {
                match k {
                    $(sem_types!(@bind_k c $variant $(($conf))?) => sem_types!(@make c SemType $variant $(($conf))?),)*
                }
            }
        }
    };
    (@pat $variant:ident ($conf:ty)) => { SemType::$variant(_) };
    (@pat $variant:ident) => { SemType::$variant };
    (@bind $c:ident $variant:ident ($conf:ty)) => { SemType::$variant($c) };
    (@bind $c:ident $variant:ident) => { SemType::$variant };
    (@bind_k $c:ident $variant:ident ($conf:ty)) => { Known::$variant($c) };
    (@bind_k $c:ident $variant:ident) => { Known::$variant };
    (@make $c:ident $e:ident $variant:ident ($conf:ty)) => { $e::$variant($c) };
    (@make $c:ident $e:ident $variant:ident) => { $e::$variant };
}

sem_types! {
    Text "text" (TextC),
    Integer "integer" (IntegerC),
    Bool "bool",
    Enum "enum" (EnumC),
    Ip "ip" (IpC),
    Port "port",
    IpPort "ip_port" (IpPortC),
    PortRange "port_range" (PortRangeC),
    Target "target" (TargetC),
    Host "host",
    ServerAddress "server_address",
    Cidr "cidr",
    CidrRule "cidr_rule",
    Duration "duration" (DurationC),
    Size "size" (SizeC),
    Rate "rate",
    Ref "ref" (RefC),
    RefList "ref_list" (RefListC),
    RefMap "ref_map" (RefMapC),
    Name "name" (NameC),
    SecretRef "secret_ref" (SecretRefC),
    Path "path" (PathC),
    UrlPath "url_path",
    Object "object" (ObjectC),
    Map "map" (MapC),
    List "list" (ListC),
    Union "union" (UnionC),
    Optional "optional" (OptionalC),
}

impl Default for SemType {
    fn default() -> Self {
        SemType::Text(TextC::default())
    }
}

impl Serialize for SemType {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.known() {
            Some(k) => k.serialize(s),
            None => match self {
                SemType::Unknown(u) => u.raw.serialize(s),
                _ => Err(serde::ser::Error::custom("unreachable semantic type")),
            },
        }
    }
}

impl<'de> Deserialize<'de> for SemType {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = Value::deserialize(d)?;
        let Some(t) = raw.get("t").and_then(Value::as_str).map(str::to_string) else {
            return Err(D::Error::custom("a semantic type needs a \"t\" tag"));
        };
        if SemType::KNOWN_TAGS.contains(&t.as_str())
            && let Ok(k) = serde_json::from_value::<Known>(raw.clone())
        {
            return Ok(SemType::from_known(k));
        }
        let fallback = raw
            .get("fallback")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|i| serde_json::from_value::<SemType>(i.clone()).ok())
                    .collect()
            })
            .unwrap_or_default();
        Ok(SemType::Unknown(UnknownType { t, fallback, raw }))
    }
}

impl SemType {
    pub fn reserved(&self) -> &[String] {
        match self {
            SemType::Ref(r) => &r.reserved,
            SemType::RefList(r) => &r.reserved,
            SemType::RefMap(r) => &r.reserved,
            SemType::Optional(o) => o.inner.sem.reserved(),
            _ => &[],
        }
    }

    pub fn is_scalar(&self) -> bool {
        match self {
            SemType::Object(_)
            | SemType::Map(_)
            | SemType::List(_)
            | SemType::Union(_)
            | SemType::RefMap(_)
            | SemType::RefList(_)
            | SemType::SecretRef(_) => false,
            SemType::Optional(o) => o.inner.sem.is_scalar(),
            SemType::Unknown(u) => u.fallback.first().is_none_or(SemType::is_scalar),
            SemType::Text(_)
            | SemType::Integer(_)
            | SemType::Bool
            | SemType::Enum(_)
            | SemType::Ip(_)
            | SemType::Port
            | SemType::IpPort(_)
            | SemType::PortRange(_)
            | SemType::Target(_)
            | SemType::Host
            | SemType::ServerAddress
            | SemType::Cidr
            | SemType::CidrRule
            | SemType::Duration(_)
            | SemType::Size(_)
            | SemType::Rate
            | SemType::Ref(_)
            | SemType::Name(_)
            | SemType::Path(_)
            | SemType::UrlPath => true,
        }
    }
}

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
