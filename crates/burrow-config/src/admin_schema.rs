use std::collections::BTreeMap;

use burrow_admin_proto::schema::{Entity, EntityInfo, Node};

use crate::field::Entry;
use crate::model::{ClientEntry, ServiceEntry, VisitorEntry};

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

pub fn type_bounds(node: &mut Node, min: i128, max: i128) {
    use burrow_admin_proto::schema::SemType;
    let fit = |v: i128| v.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64;
    if let SemType::Integer(c) = &mut node.sem {
        c.min = c.min.or(Some(fit(min)));
        c.max = c.max.or(Some(fit(max)));
    }
}

pub trait AdminEnum: Sized {
    fn options() -> Vec<burrow_admin_proto::schema::EnumOption>;
    fn wire(&self) -> &'static str;
    fn from_wire(s: &str) -> Option<Self>;
}

#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot carry the semantic type `{S}`",
    label = "this field's Rust type does not fit the `ty = ...` in #[admin(...)]",
    note = "pick a semantic type that fits the field, or add a Carrier impl in crates/burrow-config/src/admin_schema.rs"
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
        HostName,
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
        Upload,
        PasswordHash,
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
carriers!(HostName: String);
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
carriers!(Upload: String);
carriers!(PasswordHash: String);
carriers!(UrlPath: String);
carriers!(SecretRef: String);
impl<V> Carrier<sem::RefMap> for BTreeMap<String, V> {}
impl<V> Carrier<sem::Map> for BTreeMap<String, V> {}
impl<T> Carrier<sem::List> for Vec<T> {}

#[doc(hidden)]
pub mod __private {
    pub use super::{
        AdminEnum, AdminSchema, Carrier, DefaultSem, apply_range, assert_carrier, not_redundant,
        sem, type_bounds,
    };
    pub use burrow_admin_proto::schema::{
        Applies, Cond, DurationC, DurationUnit, Editable, EnumC, EnumOption, Field, Group,
        HostNameC, IntegerC, IpC, IpPortC, ListC, MapC, NameC, Node, ObjectC, OptionalC,
        PasswordHashC, PathC, PortRangeC, RefC, RefListC, RefMapC, Relation, SecretRefC, SemType,
        SizeC, TargetC, TextC, UnionC, UploadC,
    };
    use burrow_admin_proto::serde_json;
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

pub fn entities() -> Vec<EntityInfo> {
    fn info<E: Entry>(label: &str, facets: &[&str]) -> EntityInfo {
        EntityInfo {
            entity: E::kind(),
            path: vec![E::section().to_string()],
            label: label.to_string(),
            plural: E::kind().plural().to_string(),
            facets: facets.iter().map(|s| s.to_string()).collect(),
        }
    }
    vec![
        info::<ClientEntry>("Client", &["online", "from", "secret_fp"]),
        info::<VisitorEntry>("Visitor", &["open"]),
        info::<ServiceEntry>("Service", &[ServiceEntry::PROTOCOL.key(), "public"]),
        EntityInfo {
            entity: Entity::Login,
            path: crate::field::logins_path(),
            label: "Login service".to_string(),
            plural: Entity::Login.plural().to_string(),
            facets: Vec::new(),
        },
    ]
}
