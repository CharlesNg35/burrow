use burrow_admin_logic::fallback::needs_newer_ui;
use burrow_admin_logic::tree::{fields, inner};
use burrow_admin_proto::Path;
use burrow_admin_proto::schema::{Field, Node, SemType};
use burrow_admin_proto::serde_json::Value;
use burrow_config::{ServiceEntry, VisitorEntry};
use burrow_ui_kit::Toggle;
use leptos::prelude::*;

use crate::store::store;

use super::{
    ADMIN, ADMIN_FROM, ALLOW_FROM, BANDWIDTH, BIND, CLIENTS, DENY_FROM, HEALTH_CHECK,
    MAX_CONNECTIONS, MODE, PROTOCOL, PROXY_PROTOCOL, SERVICE_VISITORS, SERVICES, TARGET, VISITORS,
    editing_now, field_id, join, locked_hint, object_view, row,
};

#[derive(Clone, Debug, PartialEq)]
pub struct SettingsCard {
    pub key: String,
    pub title: String,
    pub fields: Vec<Field>,
    pub object: bool,
}

fn is_object(f: &Field) -> bool {
    let n = inner(&f.node);
    matches!(n.sem, SemType::Object(_)) && !needs_newer_ui(n)
}

pub fn card_fields(c: &SettingsCard) -> Vec<(Path, Field)> {
    if c.object {
        let Some(f) = c.fields.first() else {
            return Vec::new();
        };
        fields(&f.node)
            .iter()
            .map(|g| (vec![f.key.clone(), g.key.clone()], g.clone()))
            .collect()
    } else {
        c.fields
            .iter()
            .map(|f| (vec![f.key.clone()], f.clone()))
            .collect()
    }
}

fn danger_in(n: &Node) -> bool {
    n.danger_when_set || fields(n).iter().any(|f| danger_in(&f.node))
}

pub fn settings_cards(node: &Node, skip: &[&str]) -> Vec<SettingsCard> {
    let node = inner(node);
    let mut groups: Vec<(String, String)> = node
        .groups
        .iter()
        .map(|g| (g.key.clone(), g.label.clone()))
        .collect();
    let all: Vec<&Field> = fields(node)
        .iter()
        .filter(|f| !skip.contains(&f.key.as_str()))
        .collect();
    for f in &all {
        if !groups.iter().any(|(k, _)| *k == f.node.group) {
            groups.insert(0, (f.node.group.clone(), String::new()));
        }
    }
    let mut out = Vec::new();
    for (key, label) in groups {
        let mine: Vec<&Field> = all
            .iter()
            .copied()
            .filter(|f| f.node.group == key)
            .collect();
        let label = if label.is_empty() {
            "General".to_string()
        } else {
            label
        };
        if let [one] = mine.as_slice()
            && is_object(one)
        {
            out.push(SettingsCard {
                key,
                title: label,
                fields: vec![(*one).clone()],
                object: true,
            });
            continue;
        }
        let scalars: Vec<Field> = mine
            .iter()
            .filter(|f| !is_object(f))
            .map(|f| (*f).clone())
            .collect();
        if !scalars.is_empty() {
            out.push(SettingsCard {
                key: key.clone(),
                title: label.clone(),
                fields: scalars,
                object: false,
            });
        }
        for f in mine.into_iter().filter(|f| is_object(f)) {
            out.push(SettingsCard {
                key: format!("{key}.{}", f.key),
                title: f.node.label.clone(),
                fields: vec![f.clone()],
                object: true,
            });
        }
    }
    out
}

pub fn settings_card(c: &SettingsCard) -> AnyView {
    let locked_all = store().schema.with_untracked(|sc| {
        sc.as_ref().is_some_and(|sc| {
            let locked = sc.locked_paths();
            card_fields(c)
                .iter()
                .all(|(p, _)| locked.iter().any(|l| p.starts_with(l)))
        })
    });
    let danger = !locked_all && c.fields.iter().any(|f| danger_in(&f.node));
    let border = if danger {
        "border-danger/45"
    } else {
        "border-border"
    };
    let head = |title: String, right: AnyView| {
        view! {
            <div class="flex flex-wrap items-center gap-3 border-b border-border px-4 py-3">
                <h3 class="text-[14px] font-bold">{title}</h3>
                <span class="ml-auto"></span>
                {right}
            </div>
        }
    };
    if !c.object {
        let rows = c
            .fields
            .iter()
            .map(|f| {
                row(
                    f.clone(),
                    vec![f.key.clone()],
                    vec![f.key.clone()],
                    Vec::new(),
                )
            })
            .collect_view();
        return view! {
            <section class=format!("mb-4 scroll-mt-20 rounded-card border bg-surface {border}") data-card=c.key.clone() data-group=c.key.clone()>
                {head(c.title.clone(), ().into_any())}
                <div class="divide-y divide-rowline px-4">{rows}</div>
            </section>
        }
        .into_any();
    }
    let Some(f) = c.fields.first().cloned() else {
        return ().into_any();
    };
    let s = store();
    let e = s.edit;
    let path: Path = vec![f.key.clone()];
    let lock = s
        .schema
        .get_untracked()
        .is_some_and(|sc| sc.locked_paths().iter().any(|l| path.starts_with(l)));
    let can = s.can_write_now() && f.node.editable.is_yes() && !lock;
    let editing = editing_now();
    let optional = matches!(f.node.sem, SemType::Optional(_));
    let inner_node = inner(&f.node).clone();
    let p2 = path.clone();
    let is_set = Memo::new(move |_| e.value(&p2).is_some_and(|v| !v.is_null()));
    let (p3, tid) = (path.clone(), field_id(&path));
    let toggle = move || {
        (optional && can && editing.try_get().unwrap_or(false)).then(|| {
            let p3 = p3.clone();
            let id = tid.clone();
            view! {
                <Toggle
                    id=id
                    checked=is_set
                    label=Signal::derive(move || if is_set.get() { "In use".to_string() } else { "Not set".to_string() })
                    on_change=move |on: bool| {
                        e.set(&p3, on.then(|| Value::Object(Default::default())));
                    }
                />
            }
        })
    };
    let right = view! {
        <span class="contents">
            {lock.then(|| view! {
                <span class="flex items-center gap-1.5 text-xs text-text-muted" data-testid="locked-hint">
                    <burrow_ui_kit::Icon name="lock" small=true/>
                    {locked_hint(&f.node)}
                </span>
            })}
            {toggle}
        </span>
    }
    .into_any();
    let none_label = f
        .node
        .none_label
        .clone()
        .unwrap_or_else(|| "Not set".into());
    let help = f.node.help.clone();
    let p4 = path.clone();
    let body = move || {
        if !optional || is_set.get() {
            object_view(&inner_node, p4.clone(), p4.clone(), Vec::new())
        } else {
            view! {
                <div class="py-3">
                    <p class="text-sm" data-testid="not-set">{none_label.clone()}</p>
                    <p class="mt-1 text-xs text-text-muted">{help.clone()}</p>
                </div>
            }
            .into_any()
        }
    };
    let (jp, js) = (join(&path), join(&path));
    view! {
        <section class=format!("mb-4 scroll-mt-20 rounded-card border bg-surface {border}") data-card=c.key.clone() data-group=c.key.clone() data-path=jp data-schema=js>
            {head(c.title.clone(), right)}
            <div class="px-4">{body}</div>
        </section>
    }
    .into_any()
}

pub(super) fn layout_order(spath: &[String], group: &str) -> Option<&'static [&'static str]> {
    match (spath.first().map(String::as_str), spath.len(), group) {
        (Some(SERVICES), 2, ServiceEntry::GROUP_WHO) => Some(&[CLIENTS, MODE, HEALTH_CHECK]),
        (Some(SERVICES), 2, ServiceEntry::GROUP_EXPOSURE) => Some(&[PROTOCOL, BIND, TARGET]),
        (Some(SERVICES), 2, ServiceEntry::GROUP_ACCESS) => {
            Some(&[SERVICE_VISITORS, DENY_FROM, ALLOW_FROM])
        }
        (Some(SERVICES), 2, ServiceEntry::GROUP_TRAFFIC) => {
            Some(&[BANDWIDTH, MAX_CONNECTIONS, PROXY_PROTOCOL])
        }
        (Some(VISITORS), 2, VisitorEntry::GROUP_ADMIN) => Some(&[ADMIN, ADMIN_FROM]),
        _ => None,
    }
}
