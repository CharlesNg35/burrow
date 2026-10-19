use std::collections::BTreeMap;

use burrow::config::admin_schema::AdminSchema;
use burrow::config::model::{Bind, Reach, Secrets, Target};

#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize, AdminSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Speed {
    #[default]
    #[admin(label = "Slow", help = "Gentle")]
    SlowLane,
    #[admin(label = "Fast", danger)]
    FastLane,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, AdminSchema)]
#[admin(label = "Inner")]
pub struct Inner {
    #[admin(ty = duration(ms), label = "Delay")]
    pub delay_ms: u32,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, AdminSchema)]
#[serde(untagged)]
#[admin(semantic = union(bool, object(Inner)))]
pub enum Toggle {
    #[admin(label = "on")]
    On(bool),
    #[admin(label = "tuned")]
    Tuned(Inner),
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize, AdminSchema)]
#[serde(default)]
#[admin(entity = service, label = "Thing", groups(main = "Main", more = "More"))]
pub struct Thing {
    #[admin(ty = text(max_len = 10, pattern_hint = "abc"), group = main, order = 1)]
    pub text: String,
    #[admin(ty = integer(unit = "things"), advanced, readonly = "fixed")]
    pub count: u32,
    #[admin(danger_when_set, applies = live)]
    pub on: bool,
    #[admin(ty = ip)]
    pub ip: String,
    #[admin(ty = port, applies = restart)]
    pub port: u16,
    #[admin(ty = port_range(max = 10))]
    pub range: String,
    #[admin(ty = host)]
    pub host: String,
    #[admin(ty = server_address, group = more)]
    pub address: String,
    #[admin(ty = cidr)]
    pub cidr: String,
    #[admin(ty = cidr_rule)]
    pub rule: String,
    #[admin(ty = size(unit = "bytes"))]
    pub size: u64,
    #[admin(ty = rate, applies = reload)]
    pub rate: String,
    #[admin(ty = ref_(client), one_of = "who")]
    pub client: String,
    #[admin(ty = ref_list(client, ordered, max = 3), one_of = "who", visible_if(all(set(client), not(eq(on, true)))))]
    pub clients: Vec<String>,
    #[admin(ty = name(visitor), enabled_if(any(ne(port, 1), len_ge(clients, 2)), "why"))]
    pub name: String,
    #[admin(ty = path(must_exist, relative_to = "config"), default_from("/limits/x"))]
    pub path: String,
    #[admin(ty = url_path, warn_if(eq(text, "x"), "careful"))]
    pub url: String,
    #[admin(ty = list(cidr), empty_label = "Nobody", cross(cidr, rule), same_len(range))]
    pub list: Vec<String>,
    #[admin(ty = enum_, options_filter(eq(protocol, "tcp")), inverse(entity = visitor, label = "Back"))]
    pub speed: Speed,
    #[admin(ty = enum_values("a", "b"))]
    pub letter: String,
    #[admin(skip = "managed elsewhere")]
    pub hidden: String,
}

#[derive(serde::Serialize, AdminSchema)]
pub struct Holder {
    #[admin(label = "Plain number")]
    pub plain: u32,
    #[admin(label = "Inferred object", help = "Its help text")]
    pub inferred: Option<Inner>,
    #[admin(label = "Inferred enum")]
    pub speed: Speed,
    #[admin(ty = ip_port(ranges, max_range = 10, default_ip = "127.0.0.1"), none_label = "Off")]
    pub bind: Option<Bind>,
    #[admin(ty = target(ranges, unix))]
    pub target: Target,
    #[admin(ty = secret_ref(max = 2))]
    pub secret: Secrets,
    #[admin(ty = ref_map(client))]
    pub reach: BTreeMap<String, Reach>,
    #[admin(ty = ref_map(client, value = list(cidr_rule)))]
    pub reach2: BTreeMap<String, Vec<String>>,
    #[admin(ty = map(service))]
    pub things: BTreeMap<String, Thing>,
    #[admin(ty = union_)]
    pub toggle: Option<Toggle>,
    #[admin(ty = object)]
    pub inner: Inner,
    #[admin(ty = duration(secs))]
    pub every: u64,
}

#[derive(AdminSchema)]
#[admin(skip = "edited on that machine")]
pub struct Elsewhere {
    pub anything: u8,
}

fn main() {
    let n = <Holder as AdminSchema>::node();
    assert!(n.field("things").is_some());
    let t = <Thing as AdminSchema>::node();
    assert_eq!(t.groups.len(), 2);
    let _ = <Elsewhere as AdminSchema>::node();
}
