use std::collections::BTreeMap;

use burrow_admin_proto::schema::{Entity, EntityInfo, Node, Schema};

pub use burrow_admin_derive::AdminSchema;

pub trait AdminSchema {
    fn node() -> Node;
}

pub trait DefaultSem {
    const SEM: &'static str;
    fn default_node() -> Node;
}

macro_rules! default_sem {
    ($tag:literal => $sem:expr; $($t:ty),*) => {
        $(impl DefaultSem for $t {
            const SEM: &'static str = $tag;
            fn default_node() -> Node {
                Node::new($sem)
            }
        })*
    };
}

default_sem!("bool" => burrow_admin_proto::schema::SemType::Bool; bool);
default_sem!("integer" => burrow_admin_proto::schema::SemType::Integer(Default::default()); u8, u16, u32, u64, usize, i64);
default_sem!("text" => burrow_admin_proto::schema::SemType::Text(Default::default()); String);

pub const fn not_redundant(inferred: &str, given: &str, message: &str) {
    let (a, b) = (inferred.as_bytes(), given.as_bytes());
    if a.len() != b.len() {
        return;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return;
        }
        i += 1;
    }
    panic!("{}", message);
}

pub fn apply_range(node: &mut Node, min: Option<i64>, max: Option<i64>) {
    use burrow_admin_proto::schema::SemType;
    if let SemType::Integer(c) = &mut node.sem {
        c.min = c.min.or(min);
        c.max = c.max.or(max);
    }
}

pub trait AdminEnum {
    fn options() -> Vec<burrow_admin_proto::schema::EnumOption>;
    fn wire(&self) -> &'static str;
}

#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot carry the semantic type `{S}`",
    label = "this field's Rust type does not fit the `ty = ...` in #[admin(...)]",
    note = "pick a semantic type that fits the field, or add a Carrier impl in src/config/admin_schema.rs"
)]
pub trait Carrier<S> {}

pub fn assert_carrier<T: Carrier<S> + ?Sized, S>() {}

pub mod sem {
    macro_rules! markers {
        ($($m:ident),*) => { $(pub struct $m;)* };
    }
    markers!(
        Text,
        Integer,
        Bool,
        Enum,
        Ip,
        Port,
        IpPort,
        PortRange,
        Target,
        Host,
        ServerAddress,
        Cidr,
        CidrRule,
        Duration,
        Size,
        Rate,
        Ref,
        RefList,
        RefMap,
        Name,
        SecretRef,
        Path,
        UrlPath,
        Object,
        Map,
        List,
        Union
    );
}

macro_rules! carriers {
    ($s:ident: $($t:ty),*) => { $(impl Carrier<sem::$s> for $t {})* };
}

carriers!(Text: String);
carriers!(Integer: u16, u32, u64, usize, i64);
carriers!(Bool: bool);
carriers!(Enum: String);
carriers!(Ip: String);
carriers!(Port: u16);
carriers!(IpPort: String);
carriers!(PortRange: String);
carriers!(Host: String);
carriers!(ServerAddress: String);
carriers!(Cidr: String);
carriers!(CidrRule: String);
carriers!(Duration: u32, u64);
carriers!(Size: u32, u64);
carriers!(Rate: String);
carriers!(Ref: String);
carriers!(RefList: Vec<String>);
carriers!(Name: String);
carriers!(Path: String);
carriers!(UrlPath: String);
impl<V> Carrier<sem::RefMap> for BTreeMap<String, V> {}
impl<V: AdminSchema> Carrier<sem::Map> for BTreeMap<String, V> {}
impl<T> Carrier<sem::List> for Vec<T> {}

#[doc(hidden)]
pub mod __private {
    pub use super::{
        AdminEnum, AdminSchema, Carrier, DefaultSem, apply_range, assert_carrier, not_redundant,
        sem,
    };
    pub use burrow_admin_proto::schema::{
        Applies, Cond, DurationC, DurationUnit, Editable, EnumC, EnumOption, Field, Group,
        IntegerC, IpC, IpPortC, ListC, MapC, NameC, Node, ObjectC, OptionalC, PathC, PortRangeC,
        RefC, RefListC, RefMapC, Relation, SecretRefC, SemType, SizeC, TargetC, TextC, UnionC,
    };
    pub use serde_json::json;

    pub type Entity = burrow_admin_proto::schema::Entity;

    pub fn to_json<T: serde::Serialize>(v: &T) -> serde_json::Value {
        serde_json::to_value(v).unwrap_or(serde_json::Value::Null)
    }

    pub fn scalar_default(v: serde_json::Value) -> Option<serde_json::Value> {
        match v {
            serde_json::Value::Bool(_)
            | serde_json::Value::Number(_)
            | serde_json::Value::String(_) => Some(v),
            _ => None,
        }
    }
}

fn entities() -> Vec<EntityInfo> {
    let info =
        |entity: Entity, section: &str, label: &str, plural: &str, facets: &[&str]| EntityInfo {
            entity,
            path: vec![section.to_string()],
            label: label.to_string(),
            plural: plural.to_string(),
            facets: facets.iter().map(|s| s.to_string()).collect(),
        };
    vec![
        info(
            Entity::Client,
            "clients",
            "Client",
            "Clients",
            &["online", "from", "secret_fp"],
        ),
        info(
            Entity::Visitor,
            "visitors",
            "Visitor",
            "Visitors",
            &["open"],
        ),
        info(
            Entity::Service,
            "services",
            "Service",
            "Services",
            &["protocol", "public"],
        ),
    ]
}

pub fn server_schema() -> Schema {
    let root = <super::model::ServerConfig as AdminSchema>::node();
    let mut schema = Schema {
        schema_version: burrow_admin_proto::SCHEMA_VERSION,
        hash: String::new(),
        root,
        entities: entities(),
    };
    schema.hash = schema_hash(&schema);
    schema
}

pub fn schema_hash(schema: &Schema) -> String {
    use blake2::Digest;
    let bytes = serde_json::to_vec(&Schema {
        hash: String::new(),
        ..schema.clone()
    })
    .unwrap_or_default();
    let digest = blake2::Blake2s256::digest(&bytes);
    let hex: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!("b2s:{hex}")
}

#[cfg(test)]
mod tests;
