use burrow_admin_logic::fallback::needs_newer_ui;
use burrow_admin_logic::tree::{fields, inner};
use burrow_admin_proto::Path;
use burrow_admin_proto::schema::{Field, Node, SemType};
use burrow_config::{ServiceEntry, VisitorEntry};

use leptos::prelude::*;

use crate::store::store;

use super::more::{More, ROWS, more_settings};
use super::{
    ADMIN, ADMIN_FROM, ALLOW_FROM, BANDWIDTH, BIND, CLIENTS, DENY_FROM, HEALTH_CHECK,
    MAX_CONNECTIONS, MODE, PROTOCOL, SERVICE_VISITORS, SERVICES, TARGET, VISITORS, WEB,
    editing_now, join, locked_hint, object_view_marked, row,
};
use burrow_admin_logic::more::split;

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

fn scalar_card(c: &SettingsCard, border: &str) -> AnyView {
    let all: Vec<&Field> = c.fields.iter().collect();
    let (essential, advanced) = split(&all, false);
    let one = |f: &Field| {
        row(
            f.clone(),
            vec![f.key.clone()],
            vec![f.key.clone()],
            Vec::new(),
        )
    };
    let mut rows: Vec<AnyView> = essential.iter().map(|f| one(f)).collect();
    if !advanced.is_empty() {
        let more = More {
            fields: advanced
                .iter()
                .map(|f| ((*f).clone(), vec![f.key.clone()]))
                .collect(),
            spath: Vec::new(),
            group: c.key.clone(),
            entity: Vec::new(),
            table: true,
            alone: essential.is_empty(),
            grid: ROWS,
            title: essential.is_empty().then(|| c.title.clone()),
        };
        rows.push(more_settings(
            more,
            advanced.iter().map(|f| one(f)).collect(),
        ));
    }
    let head = (!essential.is_empty())
        .then(|| burrow_ui_kit::card_head(c.title.clone(), None, ().into_any()));
    view! {
        <section class=format!("mb-4 scroll-mt-20 rounded-card border bg-surface {border}") data-card=c.key.clone() data-group=c.key.clone()>
            {head}
            <div class="divide-y divide-rowline px-4">{rows}</div>
        </section>
    }
    .into_any()
}

fn all_advanced(node: &Node) -> bool {
    let inside = fields(inner(node));
    let one_group = inner(node).groups.len() <= 1;
    one_group
        && burrow_admin_logic::more::hides(0, inside.len())
        && (node.advanced || inside.iter().all(|f| f.node.advanced))
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
    if !c.object {
        return scalar_card(c, border);
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
    let doc = untrack(|| e.doc());
    let ports = s.schema.get_untracked().is_some_and(|sc| {
        sc.locked_paths().iter().any(|l| {
            l.len() > path.len() && l.starts_with(&path) && burrow_admin_logic::tree::holds(&doc, l)
        })
    });
    let can = s.can_write_now() && f.node.editable.is_yes() && !lock && !ports;
    let editing = editing_now();
    let optional = matches!(f.node.sem, SemType::Optional(_));
    let inner_node = inner(&f.node).clone();
    let advanced = f.node.advanced;
    let p2 = path.clone();
    let is_set = Memo::new(move |_| e.value(&p2).is_some_and(|v| !v.is_null()));
    let toggle = super::controls::object_toggle(path.clone(), is_set, optional && can, editing);
    let right = view! {
        <span class="contents">
            {lock.then(|| view! { <burrow_ui_kit::LockLine text=locked_hint(&f.node) testid="locked-hint"/> })}
            {(ports && optional).then(|| view! { <burrow_ui_kit::LockLine text=burrow_admin_logic::wording::TOGGLE_IN_FILE testid="locked-hint"/> })}
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
            object_view_marked(
                &inner_node,
                p4.clone(),
                p4.clone(),
                Vec::new(),
                &[],
                advanced,
            )
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
    let compact = !optional && !lock && all_advanced(&f.node);
    let head = (!compact).then(|| burrow_ui_kit::card_head(c.title.clone(), None, right));
    let title = super::more::CompactTitle(c.title.clone());
    view! {
        <section class=format!("mb-4 scroll-mt-20 rounded-card border bg-surface {border}") data-card=c.key.clone() data-group=c.key.clone() data-path=jp data-schema=js>
            {head}
            <div class="px-4">
                {if compact {
                    view! { <leptos::context::Provider value=title>{body}</leptos::context::Provider> }.into_any()
                } else {
                    body.into_any()
                }}
            </div>
        </section>
    }
    .into_any()
}

pub(super) fn layout_order(spath: &[String], group: &str) -> Option<&'static [&'static str]> {
    match (spath.first().map(String::as_str), spath.len(), group) {
        (Some(SERVICES), 2, ServiceEntry::GROUP_WHO) => Some(&[CLIENTS, MODE, HEALTH_CHECK]),
        (Some(SERVICES), 2, ServiceEntry::GROUP_EXPOSURE) => Some(&[PROTOCOL, TARGET]),
        (Some(SERVICES), 2, ServiceEntry::GROUP_REACH) => Some(&[BIND, WEB, SERVICE_VISITORS]),
        (Some(SERVICES), 2, ServiceEntry::GROUP_ACCESS) => {
            Some(&[ALLOW_FROM, DENY_FROM, MAX_CONNECTIONS, BANDWIDTH])
        }
        (Some(VISITORS), 2, VisitorEntry::GROUP_ADMIN) => Some(&[ADMIN, ADMIN_FROM]),
        _ => None,
    }
}
