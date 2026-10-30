mod basic;
mod keyed;
mod lists;
mod objects;
mod target_tls;
mod units;
mod website;

use burrow_admin_logic::check::{check_in, refs_exist};
use burrow_admin_logic::fallback::{Render, for_node};
use burrow_admin_logic::input::{time_unit, to_text};
use burrow_admin_logic::tree::{inner, names};
use burrow_admin_proto::Path;
use burrow_admin_proto::schema::{Entity, Node, SemType};
use burrow_admin_proto::serde_json::Value;
use burrow_config::{Field, HealthCheck, ServiceEntry};
use leptos::prelude::*;

use crate::form::object_view;
use crate::store::store;

use basic::{bool_editor, enum_editor, text_editor, yaml_editor};
use lists::{list_editor, map_editor, union_editor};
use units::{duration_editor, rate_editor};

#[derive(Clone)]
pub struct FieldCtx {
    pub node: Node,
    pub path: Path,
    pub spath: Path,
    pub entity: Path,
    pub id: String,
    pub msg_id: String,
    pub touched: RwSignal<bool>,
    pub inherited: Option<Value>,
}

pub const THIS_SERVER: &str = "This server";

pub(super) struct Choice {
    pub options: Vec<(String, String, bool)>,
    pub mode: Signal<String>,
    pub on_change: Callback<String>,
}

pub(super) fn choice_box(
    fc: &FieldCtx,
    testid: Option<&'static str>,
    choice: Choice,
    below: impl IntoView + 'static,
) -> AnyView {
    view! {
        <div id=fc.id.clone() data-testid=testid>
            <burrow_ui_kit::Segmented
                name=format!("{}-choice", fc.id)
                options=choice.options
                value=choice.mode
                label=fc.node.label.clone()
                on_change=choice.on_change
            />
            {below}
        </div>
    }
    .into_any()
}

pub fn reserved_clients() -> Vec<String> {
    store().schema.with(|s| {
        s.as_ref()
            .and_then(|s| s.root.at(&ServiceEntry::CLIENT.schema()))
            .map(|n| n.sem.reserved().to_vec())
            .unwrap_or_default()
    })
}

pub fn names_of(entity: &Entity) -> Vec<String> {
    let doc = store().edit.doc();
    names(&doc, &burrow_config::entity_path(entity))
}

pub fn set_checked(fc: &FieldCtx, node: &Node, v: Option<Value>) {
    let e = store().edit;
    if crate::grants::refuses(&fc.path, v.as_ref()) {
        e.refuse(&fc.path, true);
        return;
    }
    let problem = match &v {
        None if node.required && node.default.is_none() => {
            Some(format!("{} is required", node.label))
        }
        None => None,
        Some(x) => check_in(node, x, &e.doc())
            .err()
            .or_else(|| refs_exist(node, x, &ref_names(node)).err()),
    };
    e.local_error(&fc.path, problem);
    e.set(&fc.path, v);
}

fn ref_names(node: &Node) -> Vec<String> {
    match &inner(node).sem {
        SemType::Ref(r) => names_of(&r.entity),
        SemType::RefList(r) => names_of(&r.entity),
        _ => Vec::new(),
    }
}

pub fn editor(fc: FieldCtx) -> AnyView {
    let e = store().edit;
    let current = e.peek(&fc.path);
    let node = fc.node.clone();
    match for_node(&node, current.as_ref()) {
        Render::Raw => yaml_editor(fc),
        Render::ServerChecked => text_editor(fc, None),
        Render::Known(sem) => {
            let sem = sem.clone();
            by_sem(fc, &sem)
        }
        Render::Fallback(sem) => {
            let sem = sem.clone();
            view! { <div data-render="fallback">{by_sem(fc, &sem)}</div> }.into_any()
        }
    }
}

fn by_sem(fc: FieldCtx, sem: &SemType) -> AnyView {
    match sem {
        SemType::IpPort(c) => crate::addr::addr_editor(
            fc,
            crate::addr::Kind::Bind {
                ranges: c.ranges,
                max: c.max_range,
                default_ip: c.default_ip.clone().unwrap_or_else(|| "0.0.0.0".into()),
            },
        ),
        SemType::PortRange(c) => {
            crate::addr::addr_editor(fc, crate::addr::Kind::Range { max: c.max })
        }
        SemType::Target(c) => crate::addr::addr_editor(
            fc,
            crate::addr::Kind::Target {
                ranges: c.ranges,
                unix: c.unix,
            },
        ),
        SemType::ServerAddress => crate::addr::server_editor(fc),
        SemType::Upload(c) => crate::uploads::upload_editor(fc, c.kind.clone()),
        SemType::PasswordHash(_) => text_editor(fc, Some(sem.clone())),
        SemType::Text(_)
        | SemType::Host
        | SemType::HostName(_)
        | SemType::Path(_)
        | SemType::UrlPath
        | SemType::Name(_)
        | SemType::Ip(_)
        | SemType::Port
        | SemType::Cidr
        | SemType::CidrRule
        | SemType::Integer(_)
        | SemType::Size(_) => text_editor(fc, Some(sem.clone())),
        SemType::Bool => bool_editor(fc),
        SemType::Enum(c) => enum_editor(fc, c.clone()),
        SemType::Duration(c) => duration_editor(fc, time_unit(c.unit), c.max),
        SemType::Rate => rate_editor(fc),
        SemType::Ref(r) => crate::addr::picker(fc, r.entity.clone(), false),
        SemType::RefList(r) => crate::addr::picker(fc, r.entity.clone(), true),
        SemType::List(l) => list_editor(fc, (*l.item).clone()),
        SemType::RefMap(m) => map_editor(fc, Some(m.entity.clone()), None, (*m.value).clone()),
        SemType::Map(m) if matches!(inner(&m.value).sem, SemType::PasswordHash(_)) => {
            crate::passwords::passwords_editor(fc)
        }
        SemType::Map(m) => map_editor(fc, None, Some((*m.key).clone()), (*m.value).clone()),
        SemType::SecretRef(c) if c.reference => text_editor(fc, Some(sem.clone())),
        SemType::SecretRef(c) => crate::secrets::secret_list(fc.path.clone(), fc.id.clone(), c.max),
        SemType::Union(u) => union_editor(fc, u.clone()),
        SemType::Optional(o) => {
            let inner_node = (*o.inner).clone();
            let fc2 = FieldCtx {
                node: Node {
                    required: false,
                    reduce_only: inner_node.reduce_only || fc.node.reduce_only,
                    none_label: inner_node.none_label.clone().or(fc.node.none_label.clone()),
                    default: inner_node.default.clone().or(fc.node.default.clone()),
                    label: if inner_node.label.is_empty() {
                        fc.node.label.clone()
                    } else {
                        inner_node.label.clone()
                    },
                    help: if inner_node.help.is_empty() {
                        fc.node.help.clone()
                    } else {
                        inner_node.help.clone()
                    },
                    ..inner_node.clone()
                },
                ..fc
            };
            let sem = inner_node.sem.clone();
            by_sem(fc2, &sem)
        }
        SemType::Object(_) => {
            let node = fc.node.clone();
            object_view(&node, fc.path.clone(), fc.spath.clone(), fc.entity.clone())
        }
        SemType::Unknown(_) => text_editor(fc, None),
    }
}

fn placeholder(fc: &FieldCtx) -> String {
    fc.inherited
        .as_ref()
        .or(fc.node.default.as_ref())
        .map(|d| {
            format!(
                "Default: {}",
                d.as_u64()
                    .map(crate::format::count)
                    .unwrap_or_else(|| to_text(Some(d)))
            )
        })
        .or_else(|| fc.node.none_label.clone())
        .unwrap_or_default()
}

fn without_port(text: &str) -> &str {
    if let Some(rest) = text.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    match text.rsplit_once(':') {
        Some((head, _)) if !head.contains(':') => head,
        _ => text,
    }
}

pub fn cidr_size(text: &str) -> String {
    if text.starts_with("unix:") {
        return String::new();
    }
    let text = without_port(text);
    let (addr, bits) = match text.split_once('/') {
        Some((a, b)) => (a, b.parse::<u32>().ok()),
        None => (text, None),
    };
    let v6 = addr.contains(':');
    let max = if v6 { 128 } else { 32 };
    match bits {
        None if addr.parse::<std::net::IpAddr>().is_ok() => "single address".into(),
        Some(b) if b == max => "single address".into(),
        Some(b) if b < max && !v6 => {
            let n = 1u64 << (max - b);
            if n >= 1_000_000 {
                format!("{:.1} M addresses", n as f64 / 1_000_000.0)
            } else {
                format!("{n} addresses")
            }
        }
        Some(b) if b < max => format!("/{b} network"),
        _ => String::new(),
    }
}

pub fn health_box(node: Node, path: Path, spath: Path, entity: Path) -> AnyView {
    let e = store().edit;
    let Some(obj) = (match &inner(&node).sem {
        SemType::Union(u) => u
            .branches
            .iter()
            .find(|b| matches!(inner(b).sem, SemType::Object(_)))
            .cloned(),
        _ => None,
    }) else {
        return ().into_any();
    };
    let p = path.clone();
    let custom = Memo::new(move |_| e.value(&p).is_some_and(|v| v.is_object()));
    let p2 = path.clone();
    let summary = move || {
        let v = e.value(&p2).unwrap_or(Value::Null);
        let d = HealthCheck::default();
        let num = |f: Field<HealthCheck, u32>, d: u32| {
            f.get(&v).and_then(Value::as_u64).unwrap_or(u64::from(d))
        };
        format!(
            "= every {} s, {} s to answer, down after {} failures in a row.",
            num(HealthCheck::INTERVAL_SECS, d.interval_secs),
            num(HealthCheck::TIMEOUT_SECS, d.timeout_secs),
            num(HealthCheck::FAILURES, d.failures)
        )
    };
    (move || {
        custom.get().then(|| {
            let summary = summary.clone();
            view! {
                <div class="rounded-[10px] border border-border bg-surface px-3.5 py-3 md:col-span-2 md:[&_section]:grid-cols-3 md:[&_section]:gap-x-4 dark:bg-surface-2" data-testid="custom-box">
                    {object_view(&obj, path.clone(), spath.clone(), entity.clone())}
                    <p class="text-xs text-text-muted">{summary}</p>
                </div>
            }
        })
    })
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rule_with_a_port_is_sized_by_its_network() {
        assert_eq!(cidr_size("10.0.0.0/8"), "16.8 M addresses");
        assert_eq!(cidr_size("10.0.0.0/8:22"), "16.8 M addresses");
        assert_eq!(cidr_size("192.168.1.0/24:8000-8010"), "256 addresses");
        assert_eq!(cidr_size("192.168.1.10:22"), "single address");
        assert_eq!(cidr_size("192.168.1.10"), "single address");
        assert_eq!(cidr_size("[fd00::/64]:443"), "/64 network");
        assert_eq!(cidr_size("fd00::/64"), "/64 network");
        assert_eq!(cidr_size("unix:/run/app.sock"), "");
    }
}
