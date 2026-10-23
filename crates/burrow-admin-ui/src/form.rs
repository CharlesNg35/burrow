use burrow_admin_logic::fallback::{Render, for_node, needs_newer_ui};
use burrow_admin_logic::relations::field_state;
use burrow_admin_logic::tree::{fields, inner, merged_into, merged_single};
use burrow_admin_proto::Path;
use burrow_admin_proto::diag::Severity;
use burrow_admin_proto::schema::{Applies, DurationUnit, Editable, Field, Node, SemType};
use burrow_admin_proto::serde_json::{self, Value};
use burrow_admin_proto::value::child;
use burrow_admin_proto::value::{SecretSrc, as_env, as_secret};
use burrow_config::{Entry, ServiceEntry, VisitorEntry};
use burrow_syntax::duration::{TimeUnit, human};
use burrow_ui_kit::{Badge, Chip, FieldRow, FieldState, Toggle, Tone, chip_class};
use leptos::prelude::*;

use crate::domain::{self, FieldCtx};
use crate::edit::diags_at;
use crate::route::{Link, Route};
use crate::store::store;

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
const PROXY_PROTOCOL: &str = ServiceEntry::PROXY_PROTOCOL.key();
const ADMIN: &str = VisitorEntry::ADMIN.key();
const ADMIN_FROM: &str = VisitorEntry::ADMIN_FROM.key();

const MAX_TEXT: usize = 4096;
pub const UNREACHABLE: &str = "unreachable-service";

pub fn locked_hint(n: &Node) -> String {
    match n.applies {
        Some(Applies::Restart) => {
            "Change this in server.yaml (burrow edit), then restart the server".into()
        }
        _ => "Change this in server.yaml (burrow edit)".into(),
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
    let node = inner(node);
    let table = use_context::<ShowKeys>().is_some_and(|k| k.0);
    let mut groups: Vec<(String, String, String)> = node
        .groups
        .iter()
        .map(|g| (g.key.clone(), g.label.clone(), g.sub.clone()))
        .collect();
    let service = ServiceEntry::is_entry(&spath);
    let all: Vec<&Field> = fields(node)
        .iter()
        .filter(|f| !skip.contains(&f.key.as_str()) && merged_into(node, &f.key).is_none())
        .collect();
    for f in &all {
        if !groups.iter().any(|(k, _, _)| *k == f.node.group) {
            groups.insert(0, (f.node.group.clone(), String::new(), String::new()));
        }
    }
    let public = service.then(|| {
        let bind = child(&path, BIND);
        RwSignal::new(store().edit.peek(&bind).is_some_and(|v| !v.is_null()))
    });
    let sections = groups
        .into_iter()
        .filter_map(|(key, label, sub)| {
            let mut mine: Vec<&Field> = all.iter().copied().filter(|f| f.node.group == key).collect();
            if let Some(order) = layout_order(&spath, &key) {
                mine.sort_by_key(|f| order.iter().position(|k| *k == f.key).unwrap_or(order.len()));
            }
            let mut rows: Vec<AnyView> = Vec::new();
            for f in mine {
                if let Some(single) = merged_single(node, &f.key) {
                    let sp = join(&child(&spath, single));
                    let into = f.key.clone();
                    rows.push(view! { <span hidden data-schema=sp data-merged-into=into></span> }.into_any());
                }
                if f.key == PROTOCOL
                    && let Some(public) = public
                {
                    rows.push(visibility_row(child(&path, BIND), public));
                }
                let (fp, fs) = (child(&path, &f.key), child(&spath, &f.key));
                match (f.key.as_str(), public) {
                    (BIND, Some(public)) => {
                        let (f, entity) = ((*f).clone(), entity.clone());
                        rows.push((move || public.get().then(|| row(f.clone(), fp.clone(), fs.clone(), entity.clone()))).into_any());
                    }
                    (HEALTH_CHECK, Some(_)) => {
                        rows.push(row((*f).clone(), fp.clone(), fs.clone(), entity.clone()));
                        rows.push(domain::health_box(f.node.clone(), fp, fs, entity.clone()));
                    }
                    _ => rows.push(row((*f).clone(), fp, fs, entity.clone())),
                }
            }
            (!rows.is_empty()).then(|| {
                let n = rows.len();
                if table && path.is_empty() {
                    let label = if label.is_empty() { "General".to_string() } else { label.clone() };
                    return view! {
                        <section class="mb-4 scroll-mt-20 rounded-card border border-border bg-surface" data-group=key>
                            <div class="flex items-center border-b border-border px-4 py-3">
                                <h3 class="text-[14px] font-bold">{label}</h3>
                                <span class="ml-auto text-xs text-text-muted">{format!("{n} field{}", if n == 1 { "" } else { "s" })}</span>
                            </div>
                            <div class="divide-y divide-rowline px-4">{rows}</div>
                        </section>
                    }
                    .into_any();
                }
                if table {
                    return view! { <div class="divide-y divide-rowline" data-group=key>{rows}</div> }.into_any();
                }
                let title = (!label.is_empty()).then(|| {
                    view! {
                        <div class="flex items-baseline gap-2.5 border-b border-border pb-2.5 md:col-span-2">
                            <h3 class="text-[14px] font-bold">{label}</h3>
                            {(!sub.is_empty()).then(|| view! { <span class="text-[12.5px] text-text-muted">{sub.clone()}</span> })}
                        </div>
                    }
                });
                view! {
                    <section class="mb-[26px] grid grid-cols-1 gap-x-6 gap-y-[22px] md:grid-cols-2" data-group=key>
                        {title}
                        {rows}
                    </section>
                }
                .into_any()
            })
        })
        .collect_view();
    view! { <div class="max-w-[1120px]">{sections}</div> }.into_any()
}

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

fn layout_order(spath: &[String], group: &str) -> Option<&'static [&'static str]> {
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

fn visibility_row(bind: Path, public: RwSignal<bool>) -> AnyView {
    let e = store().edit;
    let editing = editing_now();
    let can = store().can_write_now();
    let kept: StoredValue<Option<Value>> = StoredValue::new(e.peek(&bind));
    let id = format!("{}-visibility", field_id(&bind));
    let visitors = {
        let mut p = bind.clone();
        p.pop();
        p.push(SERVICE_VISITORS.to_string());
        p
    };
    let none_picked = {
        let visitors = visitors.clone();
        Memo::new(move |_| {
            e.value(&visitors)
                .is_none_or(|v| v.as_array().is_some_and(Vec::is_empty) || v.is_null())
        })
    };
    let control = {
        let bind = bind.clone();
        let id = id.clone();
        let visitors = visitors.clone();
        move || {
            let bind = bind.clone();
            let visitors = visitors.clone();
            let on = Signal::derive(move || {
                if public.get() {
                    "public".to_string()
                } else {
                    "private".to_string()
                }
            });
            if !(can && editing.try_get().unwrap_or(false)) {
                return view! { <div class="flex min-h-[38px] items-center text-[13px]">{move || if public.get() { "Public port" } else { "Private" }}</div> }.into_any();
            }
            let to_visitors = visitors.clone();
            view! {
                <div id=id.clone()>
                    <burrow_ui_kit::Segmented
                        name=id.clone()
                        label="Visibility"
                        options=vec![("public".to_string(), "Public port".to_string(), false), ("private".to_string(), "Private".to_string(), false)]
                        value=on
                        on_change=Callback::new(move |v: String| {
                            if v == "public" {
                                public.set(true);
                                if let Some(old) = kept.get_value() {
                                    e.set(&bind, Some(old));
                                }
                            } else {
                                kept.set_value(e.peek(&bind));
                                public.set(false);
                                e.set(&bind, None);
                                if none_picked.get_untracked() {
                                    crate::screens::entity::go_to_field(&visitors);
                                }
                            }
                        })
                    />
                </div>
                {move || (!public.get() && none_picked.get()).then(|| {
                    let to = to_visitors.clone();
                    view! {
                        <p class="mt-2 flex flex-wrap items-center gap-x-2 gap-y-1 text-[12.5px] text-co-info" data-testid="pick-visitors">
                            <burrow_ui_kit::Icon name="info" small=true/>
                            "Private: only the visitors you pick reach it, with burrow visit."
                            <button type="button" class="cursor-pointer font-semibold text-accent-ink underline underline-offset-2" on:click=move |_| crate::screens::entity::go_to_field(&to)>"Pick visitors"</button>
                        </p>
                    }
                })}
            }
            .into_any()
        }
    };
    view! {
        <FieldRow id=id.clone() label="Visibility" path=format!("{}-visibility", join(&bind)) schema_path="services/*/visibility">
            {control}
        </FieldRow>
    }
    .into_any()
}

fn locked_box(node: Node, path: Path, id: String) -> AnyView {
    let e = store().edit;
    view! {
        <div class="group relative flex h-[38px] items-center rounded-input border border-border bg-surface-2 px-2.5 font-mono text-[13px] dark:bg-surface-3" id=id data-locked="">
            <span class="flex-1 truncate select-all">
                {move || {
                    let v = e.value(&path);
                    match v.or_else(|| node.default.clone()) {
                        Some(v) => locked_text(&node, &v),
                        None => node.none_label.clone().unwrap_or_else(|| "not set".into()),
                    }
                }}
            </span>
        </div>
    }
    .into_any()
}

pub fn locked_text(node: &Node, v: &Value) -> String {
    match (&inner(node).sem, v) {
        (SemType::Bool, Value::Bool(b)) => {
            if *b {
                "on".into()
            } else {
                "off".into()
            }
        }
        (SemType::Duration(c), Value::Number(n)) => n
            .as_u64()
            .map(|n| human(n, burrow_admin_logic::input::time_unit(c.unit)))
            .unwrap_or_else(|| plain(v)),
        _ => plain(v),
    }
}

fn row(f: Field, path: Path, spath: Path, entity: Path) -> AnyView {
    let s = store();
    let e = s.edit;
    let Some(schema) = s.schema.get_untracked() else {
        return ().into_any();
    };
    let node = f.node.clone();
    let st = {
        let node = node.clone();
        let entity = entity.clone();
        let schema = schema.clone();
        Memo::new(move |_| {
            e.draft
                .with(|d| field_state(&node, &schema.root, d.draft(), &entity))
        })
    };
    let lock = schema.locked_paths().iter().any(|l| spath.starts_with(l));
    let lock_here = schema.locked_paths().contains(&spath);
    let hint = locked_hint(&node);
    let can = s.can_write_now() && node.editable.is_yes() && !lock;
    let editing = editing_now();
    let show_keys = use_context::<ShowKeys>().map(|k| k.0).unwrap_or(false);
    let n = inner(&node).clone();
    if let SemType::Object(_) = n.sem
        && !needs_newer_ui(&n)
    {
        let optional = matches!(node.sem, SemType::Optional(_));
        let label = node.label.clone();
        let help = node.help.clone();
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
        let none_label = node.none_label.clone().unwrap_or_else(|| "Not set".into());
        let (p4, sp4, en4) = (path.clone(), spath.clone(), entity.clone());
        let body = move || {
            if !optional || is_set.get() {
                object_view(&n, p4.clone(), sp4.clone(), en4.clone())
            } else {
                view! { <p class="text-sm text-text-muted" data-testid="not-set">{none_label.clone()}</p> }
                    .into_any()
            }
        };
        let visible = Memo::new(move |_| st.with(|s| s.visible));
        let (jp, js) = (join(&path), join(&spath));
        return (move || {
            visible.get().then(|| {
                let (toggle, body) = (toggle.clone(), body.clone());
                view! {
                    <fieldset class="rounded-[10px] border border-border bg-surface px-3.5 py-3 md:col-span-2 dark:bg-surface-2" data-path=jp.clone() data-schema=js.clone()>
                        <legend class="px-1 text-sm font-semibold">{label.clone()}</legend>
                        <div class="mb-3 flex flex-wrap items-center gap-3">
                            <p class="flex-1 text-xs text-text-muted">{help.clone()}</p>
                            {lock_here.then(|| view! {
                                <p class="flex items-center gap-1.5 text-xs text-text-muted" data-testid="locked-hint">
                                    <burrow_ui_kit::Icon name="lock" small=true/>
                                    {hint.clone()}
                                </p>
                            })}
                            {toggle}
                        </div>
                        {body}
                    </fieldset>
                }
            })
        })
        .into_any();
    }
    let mut badges: Vec<(String, Tone)> = Vec::new();
    let mut aside: Vec<(String, Tone)> = Vec::new();
    if !lock {
        if show_keys {
            aside.extend(applies_tag(&node));
        } else {
            badges.extend(applies_tag(&node));
        }
    }
    if e.peek(&path).as_ref().and_then(as_env).is_some() {
        badges.push(("from environment".into(), Tone::Info));
    }
    let touched = RwSignal::new(false);
    let id = field_id(&path);
    let msg_id = format!("{id}-msg");
    let items = matches!(
        inner(&node).sem,
        SemType::List(_) | SemType::RefList(_) | SemType::Map(_) | SemType::RefMap(_)
    );
    let state = {
        let path = path.clone();
        let node = node.clone();
        Signal::derive(move || {
            let s = st.get();
            let picked = touched.get();
            let server = e.plan.with(|p| {
                diags_at(p.as_deref(), &path, items)
                    .into_iter()
                    .filter(|d| picked || d.code != UNREACHABLE)
                    .map(|d| (d.severity, d.message.clone()))
                    .collect::<Vec<_>>()
            });
            let server_error = server
                .iter()
                .find(|(sev, _)| *sev == Severity::Error)
                .map(|(_, m)| m.clone());
            let server_warning = server
                .iter()
                .find(|(sev, _)| *sev == Severity::Warning)
                .map(|(_, m)| m.clone());
            let related = e.related.with(|r| r.get(&path).cloned());
            let local = touched
                .get()
                .then(|| e.local.with(|l| l.get(&path).cloned()))
                .flatten()
                .or(related);
            FieldState {
                error: local.or(server_error),
                warning: server_warning.or(s.warnings.first().cloned()),
                info: None,
                disabled_reason: (!s.enabled).then(|| s.disabled_reason.clone()).flatten(),
                locked_reason: if lock {
                    Some(locked_hint(&node))
                } else {
                    locked(&node)
                },
                modified: e.draft.with(|d| d.modified(&path)),
            }
        })
    };
    let body = {
        let (path, spath, entity, node) =
            (path.clone(), spath.clone(), entity.clone(), node.clone());
        let id = id.clone();
        let msg_id = msg_id.clone();
        move || {
            let enabled = st.with(|s| s.enabled);
            if lock && editing.try_get().unwrap_or(false) {
                return locked_box(node.clone(), path.clone(), id.clone());
            }
            if !lock && can && enabled && editing.try_get().unwrap_or(false) {
                let fc = FieldCtx {
                    node: node.clone(),
                    path: path.clone(),
                    spath: spath.clone(),
                    entity: entity.clone(),
                    id: id.clone(),
                    msg_id: msg_id.clone(),
                    touched,
                    inherited: st.with_untracked(|s| s.default_from.clone()),
                };
                view! {
                    <div class="flex flex-wrap items-start gap-2" data-editor="">
                        <div class="min-w-0 flex-1">{domain::editor(fc)}</div>
                        {reset_button(path.clone())}
                    </div>
                    {crate::grants::refused_line(path.clone())}
                }
                .into_any()
            } else {
                let node = node.clone();
                let path = path.clone();
                let single = crate::addr::merged_partner(&spath).map(|s| {
                    let mut p = path.clone();
                    p.pop();
                    p.push(s);
                    p
                });
                let inherited = st.with_untracked(|s| s.default_from.clone());
                view! {
                    <div class="flex min-h-[38px] items-center text-[13px]" id=id.clone()>
                        {move || {
                            let v = e.value(&path).or_else(|| {
                                single
                                    .as_ref()
                                    .and_then(|s| e.value(s))
                                    .map(|c| Value::Array(vec![c]))
                            });
                            match ctx_now() {
                                Some(c) => value_view(&node, v.as_ref(), inherited.as_ref(), &c),
                                None => ().into_any(),
                            }
                        }}
                    </div>
                }
                .into_any()
            }
        }
    };
    let label = node.label.clone();
    let help = if lock_here {
        format!("{}.", locked_hint(&node))
    } else {
        node.help.clone()
    };
    let key_hint = if show_keys {
        spath.join(".\u{200b}")
    } else if matches!(inner(&node).sem, SemType::List(_)) {
        f.key.clone()
    } else {
        String::new()
    };
    let wide = node.wide.unwrap_or_else(|| is_wide(&node));
    let saved_line = matches!(
        inner(&node).sem,
        SemType::IpPort(_) | SemType::PortRange(_) | SemType::Target(_)
    );
    let visible = Memo::new(move |_| st.with(|s| s.visible));
    let (jp, js) = (join(&path), join(&spath));
    (move || {
        visible.get().then(|| {
            let body = body.clone();
            view! {
                <FieldRow
                    id=id.clone()
                    label=label.clone()
                    help=help.clone()
                    hide_help=Signal::derive(move || saved_line && editing.try_get().unwrap_or(false))
                    path=jp.clone()
                    schema_path=js.clone()
                    key_hint=key_hint.clone()
                    badges=badges.clone()
                    aside=aside.clone()
                    table=show_keys
                    wide=wide
                    state=state
                >
                    {body}
                </FieldRow>
            }
        })
    })
    .into_any()
}

fn reset_button(path: Path) -> impl IntoView {
    let e = store().edit;
    let modified = {
        let path = path.clone();
        Memo::new(move |_| e.draft.with(|d| d.modified(&path)))
    };
    move || {
        let path = path.clone();
        modified.get().then(|| {
            view! {
                <button
                    type="button"
                    class="absolute top-0 right-0 flex h-6 cursor-pointer items-center gap-1 rounded-sm px-1.5 text-xs font-semibold text-text-muted hover:bg-surface-3 hover:text-text"
                    title="Reset field"
                    aria-label="Reset field"
                    data-testid="reset-field"
                    on:click=move |_| {
                        let base = e
                            .draft
                            .with_untracked(|d| burrow_admin_proto::value::get(d.base(), &path).cloned());
                        e.local_error(&path, None);
                        match base {
                            Some(b) => e.set(&path, Some(b)),
                            None => e.remove_pruned(&path),
                        }
                    }
                >
                    <burrow_ui_kit::Icon name="revert" small=true/>
                    "Reset"
                </button>
            }
        })
    }
}

fn object_static(node: &Node, v: &Value, ctx: &Ctx) -> AnyView {
    let rows = fields(inner(node))
        .iter()
        .filter_map(|f| {
            let x = v.get(&f.key)?;
            Some(view! {
                <tr>
                    <th scope="row">{f.node.label.clone()}</th>
                    <td>{value_view(&f.node, Some(x), None, ctx)}</td>
                </tr>
            })
        })
        .collect_view();
    view! { <table class="text-sm [&_td]:py-1 [&_td]:pl-3 [&_th]:py-1 [&_th]:text-left [&_th]:font-normal"><tbody>{rows}</tbody></table> }.into_any()
}

fn raw_view(v: Option<&Value>) -> AnyView {
    let text = v
        .map(|v| cap(serde_json::to_string_pretty(v).unwrap_or_default()))
        .unwrap_or_else(|| "Not set".into());
    view! {
        <div class="rounded-card border border-dashed border-warning p-2" data-testid="raw-node">
            <p class="text-sm text-warning">"This setting needs a newer burrow admin to edit it in a form."</p>
            <pre class="mt-1 font-mono text-sm whitespace-pre-wrap">{text}</pre>
        </div>
    }
    .into_any()
}

fn server_checked() -> AnyView {
    view! { <Badge text="Checked by the server" tone=Tone::Info/> }.into_any()
}

fn value_view(node: &Node, v: Option<&Value>, inherited: Option<&Value>, ctx: &Ctx) -> AnyView {
    let v = v.filter(|v| !v.is_null());
    let Some(v) = v else {
        if matches!(inner(node).sem, SemType::Unknown(_)) && for_node(node, None) == Render::Raw {
            return raw_view(None);
        }
        let fallback = inherited.or(node.default.as_ref());
        return match fallback {
            Some(d) => {
                let shown = value_view(node, Some(d), None, ctx);
                view! { <span class="inline-flex flex-wrap items-center gap-1.5" data-unset="default">{shown}<span class="text-text-muted">"(default)"</span></span> }.into_any()
            }
            None => {
                let text = node.none_label.clone().unwrap_or_else(|| "Not set".into());
                view! { <span class="text-text-muted" data-unset="none">{text}</span> }.into_any()
            }
        };
    };
    if let Some(e) = as_env(v) {
        let resolved = e
            .resolved
            .map(|r| format!(" = {}", cap(r)))
            .unwrap_or_default();
        return view! {
            <span class="font-mono text-sm">{cap(e.raw)}</span>
            <span class="text-text-muted">{resolved}</span>
        }
        .into_any();
    }
    match for_node(node, Some(v)) {
        Render::Known(sem) => sem_view(sem, node, v, ctx),
        Render::Fallback(sem) => view! {
            <span class="inline-flex flex-wrap items-center gap-2" data-render="fallback">
                {sem_view(sem, node, v, ctx)}
                {server_checked()}
            </span>
        }
        .into_any(),
        Render::ServerChecked => view! {
            <span class="inline-flex flex-wrap items-center gap-2" data-render="server-checked">
                <span class="font-mono text-sm">{plain(v)}</span>
                {server_checked()}
            </span>
        }
        .into_any(),
        Render::Raw => raw_view(Some(v)),
    }
}

fn mono(v: &Value) -> AnyView {
    view! { <span class="font-mono text-sm">{plain(v)}</span> }.into_any()
}

fn ref_link(section: &str, name: &str, reserved: &[String], ctx: &Ctx) -> AnyView {
    if reserved.iter().any(|r| r == name) {
        let to = Route::Settings;
        let local = name.to_string();
        return view! {
            <Link to=to class=chip_class(Tone::Accent, true)>
                <span data-local=local>{crate::domain::THIS_SERVER}</span>
            </Link>
        }
        .into_any();
    }
    let exists = ctx.doc.get(section).and_then(|m| m.get(name)).is_some();
    let name = name.to_string();
    if exists {
        let to = Route::Detail(section.to_string(), name.clone());
        view! { <Link to=to class=chip_class(Tone::Accent, true)>{name}</Link> }.into_any()
    } else {
        view! { <Chip text=name mono=true tone=Tone::Danger/> }.into_any()
    }
}

fn names(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a.iter().map(plain).collect(),
        other => vec![plain(other)],
    }
}

fn sem_view(sem: &SemType, node: &Node, v: &Value, ctx: &Ctx) -> AnyView {
    match sem {
        SemType::Text(_)
        | SemType::Host
        | SemType::ServerAddress
        | SemType::Path(_)
        | SemType::UrlPath
        | SemType::Name(_) => mono(v),
        SemType::Ip(_)
        | SemType::Port
        | SemType::IpPort(_)
        | SemType::PortRange(_)
        | SemType::Target(_) => mono(v),
        SemType::Integer(c) => {
            let unit = c.unit.clone().map(|u| format!(" {u}")).unwrap_or_default();
            let n = v.as_u64().map(crate::format::count).unwrap_or_else(|| plain(v));
            view! { <span class="tabular-nums">{format!("{n}{unit}")}</span> }.into_any()
        }
        SemType::Bool => {
            let yes = v.as_bool().unwrap_or(false);
            view! { <span class="font-medium" data-value=yes.to_string()>{if yes { "Yes" } else { "No" }}</span> }.into_any()
        }
        SemType::Enum(c) => {
            let raw = plain(v);
            let opt = c.options.iter().find(|o| o.value == raw);
            let label = opt.map(|o| o.label.clone()).unwrap_or(raw.clone());
            let class = if opt.is_some_and(|o| o.danger) {
                "font-medium text-danger"
            } else {
                "font-medium"
            };
            view! { <span class=class title=raw data-value=plain(v)>{label}</span> }.into_any()
        }
        SemType::Cidr | SemType::CidrRule => view! { <Chip text=plain(v) mono=true/> }.into_any(),
        SemType::Duration(c) => {
            let unit = match c.unit {
                DurationUnit::Ms => TimeUnit::Ms,
                DurationUnit::Secs | DurationUnit::Unknown => TimeUnit::Secs,
            };
            match v.as_u64() {
                Some(n) => {
                    view! { <span class="tabular-nums" title=plain(v)>{human(n, unit)}</span> }
                        .into_any()
                }
                None => mono(v),
            }
        }
        SemType::Size(c) => {
            view! { <span class="tabular-nums">{format!("{} {}", plain(v), c.unit)}</span> }
                .into_any()
        }
        SemType::Rate => {
            let raw = plain(v);
            let shown = burrow_syntax::rate::parse_rate(&raw)
                .map(|b| format!("{raw} ({})", burrow_syntax::rate::describe(b)))
                .unwrap_or(raw);
            view! { <span class="font-mono text-sm">{shown}</span> }.into_any()
        }
        SemType::Ref(c) => ref_link(c.entity.section(), &plain(v), &c.reserved, ctx),
        SemType::RefList(c) => {
            let section = c.entity.section().to_string();
            let items = names(v);
            if items.is_empty() {
                return view! { <span class="text-text-muted">"None"</span> }.into_any();
            }
            view! {
                <span class="flex flex-wrap gap-1">
                    {items.iter().map(|n| ref_link(&section, n, &c.reserved, ctx)).collect_view()}
                </span>
            }
            .into_any()
        }
        SemType::RefMap(c) => {
            let section = c.entity.section().to_string();
            let Some(m) = v.as_object() else {
                return mono(v);
            };
            let rows = m
                .iter()
                .map(|(k, x)| {
                    view! {
                        <tr>
                            <th scope="row">{ref_link(&section, k, &c.reserved, ctx)}</th>
                            <td>{value_view(&c.value, Some(x), None, ctx)}</td>
                        </tr>
                    }
                })
                .collect_view();
            view! { <table class="text-sm [&_td]:py-1 [&_td]:pl-3 [&_th]:py-1 [&_th]:text-left [&_th]:font-normal"><tbody>{rows}</tbody></table> }.into_any()
        }
        SemType::SecretRef(_) => {
            let items: Vec<Value> = match v {
                Value::Array(a) => a.clone(),
                other => vec![other.clone()],
            };
            let rows = items
                .iter()
                .map(|i| match as_secret(i) {
                    Some(s) => {
                        let fp = s.fp.clone().unwrap_or_else(|| "unset".into());
                        let src = match &s.src {
                            SecretSrc::Literal => "in the file".to_string(),
                            SecretSrc::Env(e) => format!("from ${{{e}}}"),
                        };
                        view! {
                            <li class="flex flex-wrap gap-2" data-fp=fp.clone()>
                                <span class="font-mono text-sm">{format!("secret {fp}")}</span>
                                <span class="text-text-muted">{src}</span>
                            </li>
                        }
                        .into_any()
                    }
                    None => view! { <li class="font-mono text-sm">"hidden"</li> }.into_any(),
                })
                .collect_view();
            view! { <ul class="space-y-1">{rows}</ul> }.into_any()
        }
        SemType::List(c) => {
            let Some(a) = v.as_array() else {
                return mono(v);
            };
            if a.is_empty() {
                return view! { <span class="text-text-muted">"None"</span> }.into_any();
            }
            let item = (*c.item).clone();
            view! {
                <span class="flex flex-wrap gap-1">
                    {a.iter().map(|x| value_view(&item, Some(x), None, ctx)).collect_view()}
                </span>
            }
            .into_any()
        }
        SemType::Map(c) => {
            let Some(m) = v.as_object() else {
                return mono(v);
            };
            let rows = m
                .iter()
                .map(|(k, x)| {
                    view! {
                        <tr>
                            <th scope="row" class="font-mono text-sm">{k.clone()}</th>
                            <td>{value_view(&c.value, Some(x), None, ctx)}</td>
                        </tr>
                    }
                })
                .collect_view();
            view! { <table class="text-sm [&_td]:py-1 [&_td]:pl-3 [&_th]:py-1 [&_th]:text-left [&_th]:font-normal"><tbody>{rows}</tbody></table> }.into_any()
        }
        SemType::Union(u) if u.branches.iter().any(|b| matches!(inner(b).sem, SemType::Bool)) && !matches!(v, Value::String(_)) => match v {
            Value::Bool(on) => view! { <span class="font-medium" data-value=on.to_string()>{if *on { "On" } else { "Off" }}</span> }.into_any(),
            _ => {
                let object = u.branches.iter().find(|b| matches!(inner(b).sem, SemType::Object(_)));
                view! {
                    <span class="block">
                        <span class="font-medium">"Custom"</span>
                        {object.map(|b| value_view(b, Some(v), None, ctx))}
                    </span>
                }
                .into_any()
            }
        },
        SemType::Union(u) => {
            let pick = u
                .branches
                .iter()
                .find(|b| burrow_admin_logic::check::check(b, v).is_ok())
                .or(u.branches.first());
            match pick {
                Some(b) => value_view(b, Some(v), None, ctx),
                None => mono(v),
            }
        }
        SemType::Optional(o) => value_view(&o.inner, Some(v), None, ctx),
        SemType::Object(_) => object_static(node, v, ctx),
        SemType::Unknown(_) => mono(v),
    }
}
