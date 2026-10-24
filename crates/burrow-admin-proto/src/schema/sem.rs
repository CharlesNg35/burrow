use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use super::{Entity, Field, Node, is_false};

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
