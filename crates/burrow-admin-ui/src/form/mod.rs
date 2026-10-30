mod cards;
mod controls;
mod more;
mod off_group;
mod reach;
mod row;
mod view;

use burrow_admin_logic::more::split;
use burrow_admin_logic::tree::{fields, inner, merged_into, merged_single};
use burrow_admin_proto::Path;
use burrow_admin_proto::schema::{Applies, Editable, Field, Node, SemType};
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::value::child;
use burrow_config::{Entry, ServiceEntry, VisitorEntry};
use burrow_ui_kit::Tone;
use leptos::prelude::*;

use crate::domain;
use crate::store::store;

use cards::layout_order;
pub(crate) use cards::{SettingsCard, card_fields, settings_card, settings_cards};
use more::{More, more_settings};
pub use more::{PageSearch, reveal};
use row::row;
pub(crate) use view::server_checked;
use view::value_view;

const SERVICES: &str = ServiceEntry::SECTION;
const VISITORS: &str = VisitorEntry::SECTION;
const CLIENTS: &str = ServiceEntry::CLIENTS.key();
const MODE: &str = ServiceEntry::MODE.key();
const HEALTH_CHECK: &str = ServiceEntry::HEALTH_CHECK.key();
const PROTOCOL: &str = ServiceEntry::PROTOCOL.key();
const BIND: &str = ServiceEntry::BIND.key();
const TARGET: &str = ServiceEntry::TARGET.key();
const SERVICE_VISITORS: &str = ServiceEntry::VISITORS.key();
const DENY_FROM: &str = ServiceEntry::DENY_FROM.key();
const ALLOW_FROM: &str = ServiceEntry::ALLOW_FROM.key();
const BANDWIDTH: &str = ServiceEntry::BANDWIDTH.key();
const MAX_CONNECTIONS: &str = ServiceEntry::MAX_CONNECTIONS.key();
const ADMIN: &str = VisitorEntry::ADMIN.key();
const WEB: &str = ServiceEntry::WEB.key();
const SITE_HOSTS: &str = burrow_config::WebSite::HOSTS.key();
const ADMIN_FROM: &str = VisitorEntry::ADMIN_FROM.key();

const MAX_TEXT: usize = 4096;

pub fn locked_hint(n: &Node) -> String {
    match n.applies {
        Some(Applies::Restart) => burrow_admin_logic::wording::CHANGE_IN_FILE_THEN_RESTART.into(),
        _ => burrow_admin_logic::wording::CHANGE_IN_FILE.into(),
    }
}

#[derive(Clone)]
pub struct Ctx {
    pub doc: Value,
}

pub fn join(p: &[String]) -> String {
    p.join("/")
}

pub fn field_id(path: &[String]) -> String {
    format!(
        "f-{}",
        join(path).replace(|c: char| !c.is_ascii_alphanumeric(), "-")
    )
}

fn cap(s: String) -> String {
    if s.len() <= MAX_TEXT {
        return s;
    }
    let mut end = MAX_TEXT;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

pub fn plain(v: &Value) -> String {
    cap(match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    })
}

fn locked(n: &Node) -> Option<String> {
    match &n.editable {
        Editable::Yes => None,
        Editable::No { reason } if reason.is_empty() => Some("Not editable here".into()),
        Editable::No { reason } => Some(reason.clone()),
    }
}

fn ctx_now() -> Option<Ctx> {
    let s = store();
    s.schema.get_untracked()?;
    Some(Ctx { doc: s.edit.doc() })
}

#[derive(Clone, Copy)]
pub struct EditMode(pub Signal<bool>);

#[derive(Clone, Copy)]
pub struct ShowKeys(pub bool);

fn editing_now() -> Signal<bool> {
    use_context::<EditMode>()
        .map(|m| m.0)
        .unwrap_or(Signal::stored(true))
}

fn is_wide(n: &Node) -> bool {
    match &inner(n).sem {
        SemType::RefList(_)
        | SemType::List(_)
        | SemType::RefMap(_)
        | SemType::Map(_)
        | SemType::Union(_)
        | SemType::SecretRef(_) => true,
        SemType::IpPort(c) => c.ranges,
        SemType::Target(c) => c.ranges,
        _ => false,
    }
}

fn applies_tag(n: &Node) -> Option<(String, Tone)> {
    match n.applies {
        Some(Applies::Live) => Some(("live".into(), Tone::Success)),
        Some(Applies::Reload) => Some(("on reload".into(), Tone::Accent)),
        Some(Applies::Restart) => Some(("after restart".into(), Tone::Warning)),
        Some(Applies::Unknown) | None => None,
    }
}

pub fn object_view(node: &Node, path: Path, spath: Path, entity: Path) -> AnyView {
    object_view_skipping(node, path, spath, entity, &[])
}

pub fn object_view_skipping(
    node: &Node,
    path: Path,
    spath: Path,
    entity: Path,
    skip: &[&str],
) -> AnyView {
    object_view_marked(node, path, spath, entity, skip, false)
}

struct Section<'a> {
    node: &'a Node,
    path: &'a Path,
    spath: &'a Path,
    entity: &'a Path,
}

impl Section<'_> {
    fn rows(&self, mine: &[&Field]) -> Vec<AnyView> {
        let (path, spath, entity) = (self.path, self.spath, self.entity);
        let mut rows: Vec<(Option<off_group::Off>, AnyView)> = Vec::new();
        for f in mine {
            if let Some(single) = merged_single(self.node, &f.key) {
                let sp = join(&child(spath, single));
                let into = f.key.clone();
                rows.push((
                    None,
                    view! { <span hidden data-schema=sp data-merged-into=into></span> }.into_any(),
                ));
            }
            let (fp, fs) = (under(path, &f.key), under(spath, &f.key));
            if f.key.contains('/') {
                let site = fp[..fp.len() - 1].to_vec();
                rows.push((
                    off_group::off_of(&f.node, site),
                    site_row((*f).clone(), fp, fs),
                ));
                continue;
            }
            let off = off_group::off_of(&f.node, entity.clone());
            let service = ServiceEntry::is_entry(spath);
            match (f.key.as_str(), service) {
                (HEALTH_CHECK, true) => {
                    let next = off_group::beside(&off);
                    rows.push((
                        off,
                        row((*f).clone(), fp.clone(), fs.clone(), entity.clone()),
                    ));
                    rows.push((
                        next,
                        domain::health_box(f.node.clone(), fp, fs, entity.clone()),
                    ));
                }
                _ => rows.push((off, row((*f).clone(), fp, fs, entity.clone()))),
            }
        }
        off_group::assemble(rows)
    }
}

fn under(base: &[String], key: &str) -> Path {
    let mut p = base.to_vec();
    p.extend(key.split('/').map(str::to_string));
    p
}

fn site_fields(node: &Node, skip: &[&str]) -> Vec<Field> {
    let Some(web) = fields(node).iter().find(|f| f.key == WEB) else {
        return Vec::new();
    };
    if skip.contains(&WEB) {
        return Vec::new();
    }
    let SemType::Union(u) = &inner(&web.node).sem else {
        return Vec::new();
    };
    u.branches
        .iter()
        .flat_map(|b| fields(b).iter())
        .filter(|g| g.key != SITE_HOSTS)
        .map(|g| Field {
            key: format!("{WEB}/{}", g.key),
            node: Node {
                group: web.node.group.clone(),
                ..g.node.clone()
            },
        })
        .collect()
}

pub(super) fn site_named(site: &[String]) -> bool {
    store()
        .edit
        .value(site)
        .is_some_and(|v| !burrow_admin_logic::web::names(&v).is_empty())
}

fn site_row(f: Field, path: Path, spath: Path) -> AnyView {
    let site = path[..path.len() - 1].to_vec();
    let entity = site.clone();
    let named = Memo::new(move |_| site_named(&site));
    (move || {
        named
            .get()
            .then(|| row(f.clone(), path.clone(), spath.clone(), entity.clone()))
    })
    .into_any()
}

pub(crate) fn object_view_marked(
    node: &Node,
    path: Path,
    spath: Path,
    entity: Path,
    skip: &[&str],
    all_advanced: bool,
) -> AnyView {
    let node = inner(node);
    let table = use_context::<ShowKeys>().is_some_and(|k| k.0);
    let mut groups: Vec<(String, String, String)> = node
        .groups
        .iter()
        .map(|g| (g.key.clone(), g.label.clone(), g.sub.clone()))
        .collect();
    let service = ServiceEntry::is_entry(&spath);
    let site = if service {
        site_fields(node, skip)
    } else {
        Vec::new()
    };
    let all: Vec<&Field> = fields(node)
        .iter()
        .filter(|f| !skip.contains(&f.key.as_str()) && merged_into(node, &f.key).is_none())
        .chain(site.iter())
        .collect();
    for f in &all {
        if !groups.iter().any(|(k, _, _)| *k == f.node.group) {
            groups.insert(0, (f.node.group.clone(), String::new(), String::new()));
        }
    }
    let at = Section {
        node,
        path: &path,
        spath: &spath,
        entity: &entity,
    };
    let sections = groups
        .into_iter()
        .filter_map(|(key, label, sub)| {
            let mut mine: Vec<&Field> = all
                .iter()
                .copied()
                .filter(|f| f.node.group == key)
                .collect();
            if let Some(order) = layout_order(&spath, &key) {
                mine.sort_by_key(|f| {
                    order
                        .iter()
                        .position(|k| *k == f.key)
                        .unwrap_or(order.len())
                });
            }
            let (essential, advanced) = split(&mine, all_advanced);
            let n = mine.len();
            if service && key == ServiceEntry::GROUP_REACH {
                let ways = reach::reached_by(path.clone(), spath.clone(), entity.clone(), &mine);
                return Some(section_view(false, table, key, label, sub, n, vec![ways]));
            }
            let mut rows = at.rows(&essential);
            if !advanced.is_empty() {
                let more = More {
                    fields: advanced
                        .iter()
                        .map(|f| ((*f).clone(), under(&path, &f.key)))
                        .collect(),
                    spath: spath.clone(),
                    group: key.clone(),
                    entity: entity.clone(),
                    table,
                    alone: essential.is_empty(),
                    grid: more::GRID,
                    title: essential
                        .is_empty()
                        .then(|| use_context::<more::CompactTitle>().map(|t| t.0))
                        .flatten(),
                };
                rows.push(more_settings(more, at.rows(&advanced)));
            }
            let site = service && key == ServiceEntry::GROUP_WEBSITE;
            let view = (n > 0)
                .then(|| section_view(table && path.is_empty(), table, key, label, sub, n, rows));
            if site {
                let p = child(&path, WEB);
                let e = store().edit;
                let on = Memo::new(move |_| e.value(&p).is_some_and(|v| !v.is_null()));
                return view
                    .map(|v| view! { <div class:hidden=move || !on.get()>{v}</div> }.into_any());
            }
            view
        })
        .collect_view();
    let width = if table { "" } else { "max-w-[1120px]" };
    view! { <div class=width>{sections}</div> }.into_any()
}

fn section_view(
    card: bool,
    table: bool,
    key: String,
    label: String,
    sub: String,
    n: usize,
    rows: Vec<AnyView>,
) -> AnyView {
    if card {
        let label = if label.is_empty() {
            "General".to_string()
        } else {
            label
        };
        return view! {
            <section class="mb-4 scroll-mt-20 rounded-card border border-border bg-surface" data-group=key>
                {burrow_ui_kit::card_head(label, None, format!("{n} field{}", if n == 1 { "" } else { "s" }).into_any())}
                <div class="divide-y divide-rowline px-4">{rows}</div>
            </section>
        }
        .into_any();
    }
    if table {
        return view! { <div class="divide-y divide-rowline" data-group=key>{rows}</div> }
            .into_any();
    }
    let head = (!label.is_empty())
        .then(|| burrow_ui_kit::card_head(label, (!sub.is_empty()).then_some(sub), ().into_any()));
    view! {
        <section class="mb-4 scroll-mt-20 rounded-card border border-border bg-surface" data-group=key>
            {head}
            <div class="grid grid-cols-1 gap-x-6 gap-y-[22px] px-4 py-4 md:grid-cols-2">{rows}</div>
        </section>
    }
    .into_any()
}
