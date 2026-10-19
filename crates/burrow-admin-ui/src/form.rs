use std::sync::Arc;

use burrow_admin_logic::fallback::{Render, for_node, needs_newer_ui};
use burrow_admin_logic::relations::field_state;
use burrow_admin_logic::tree::{fields, inner};
use burrow_admin_proto::Path;
use burrow_admin_proto::diag::Severity;
use burrow_admin_proto::schema::{Applies, DurationUnit, Editable, Field, Node, Schema, SemType};
use burrow_admin_proto::serde_json::{self, Value};
use burrow_admin_proto::value::{SecretSrc, as_env, as_secret};
use burrow_syntax::duration::{TimeUnit, human};
use burrow_ui_kit::{Badge, Chip, FieldRow, FieldState, Toggle, Tone, button_class, chip_class};
use leptos::prelude::*;

use crate::domain::{self, FieldCtx};
use crate::edit::diags_at;
use crate::route::{Link, Route};
use crate::store::store;

const MAX_TEXT: usize = 4096;
pub const LOCKED_HINT: &str = "Change this in server.yaml on the server, then restart";

#[derive(Clone)]
pub struct Ctx {
    pub schema: Arc<Schema>,
    pub doc: Value,
}

pub fn join(p: &[String]) -> String {
    p.join("/")
}

pub fn push(p: &[String], k: &str) -> Path {
    let mut out = p.to_vec();
    out.push(k.to_string());
    out
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

pub fn ctx_now() -> Option<Ctx> {
    let s = store();
    let schema = s.schema.get_untracked()?;
    Some(Ctx {
        schema,
        doc: s.edit.doc(),
    })
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

fn tcp_only(n: &Node) -> bool {
    n.relations.iter().any(|r| match r {
        burrow_admin_proto::schema::Relation::EnabledIf { reason, .. } => reason.contains("UDP"),
        _ => false,
    })
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
    let sections = groups
        .into_iter()
        .filter_map(|(key, label)| {
            let rows: Vec<AnyView> = all
                .iter()
                .filter(|f| f.node.group == key)
                .map(|f| {
                    row(
                        (*f).clone(),
                        push(&path, &f.key),
                        push(&spath, &f.key),
                        entity.clone(),
                    )
                })
                .collect();
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
                        <div class="mb-4 border-b border-border pb-2.5 md:col-span-2">
                            <h3 class="text-[14px] font-bold">{label}</h3>
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
            <span class="text-text-muted"><burrow_ui_kit::Icon name="lock"/></span>
        </div>
    }
    .into_any()
}

fn locked_text(node: &Node, v: &Value) -> String {
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
            if is_set.get() || !optional {
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
                            {lock.then(|| view! {
                                <p class="flex items-center gap-1.5 text-xs text-text-muted" data-testid="locked-hint">
                                    <burrow_ui_kit::Icon name="lock" small=true/>
                                    {LOCKED_HINT}
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
    if tcp_only(&node) {
        badges.push(("TCP ONLY".into(), Tone::Warning));
    }
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
    let state = {
        let path = path.clone();
        let node = node.clone();
        Signal::derive(move || {
            let s = st.get();
            let server = e.plan.with(|p| {
                diags_at(p.as_deref(), &path)
                    .into_iter()
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
                    Some(LOCKED_HINT.to_string())
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
            if lock {
                return locked_box(node.clone(), path.clone(), id.clone());
            }
            if can && enabled && editing.try_get().unwrap_or(false) {
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
                }
                .into_any()
            } else {
                let node = node.clone();
                let path = path.clone();
                let inherited = st.with_untracked(|s| s.default_from.clone());
                view! {
                    <div class="flex min-h-[38px] items-center text-[13px]" id=id.clone()>
                        {move || {
                            let v = e.value(&path);
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
    let help = if lock {
        format!("{LOCKED_HINT}.")
    } else {
        node.help.clone()
    };
    let key_hint = if show_keys {
        spath.join(".")
    } else if matches!(inner(&node).sem, SemType::List(_)) {
        f.key.clone()
    } else {
        String::new()
    };
    let wide = is_wide(&node);
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
                    class=button_class("ghost", true)
                    title="Undo the change of this field"
                    data-testid="reset-field"
                    on:click=move |_| {
                        let base = e
                            .draft
                            .with_untracked(|d| burrow_admin_logic::changeset::get(d.base(), &path).cloned());
                        e.local_error(&path, None);
                        e.set(&path, base);
                    }
                >
                    "Undo"
                </button>
            }
        })
    }
}

pub fn object_static(node: &Node, v: &Value, ctx: &Ctx) -> AnyView {
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

pub fn value_view(node: &Node, v: Option<&Value>, inherited: Option<&Value>, ctx: &Ctx) -> AnyView {
    let v = v.filter(|v| !v.is_null());
    let Some(v) = v else {
        if matches!(inner(node).sem, SemType::Unknown(_)) && for_node(node, None) == Render::Raw {
            return raw_view(None);
        }
        let fallback = inherited.or(node.default.as_ref());
        return match fallback {
            Some(d) => {
                let shown = value_view(node, Some(d), None, ctx);
                view! { <span class="inline-flex flex-wrap items-center gap-1 text-text-muted" data-unset="default">"Default:" {shown}</span> }.into_any()
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

fn ref_link(section: &str, name: &str, ctx: &Ctx) -> AnyView {
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
            view! { <span class="tabular-nums">{format!("{}{unit}", plain(v))}</span> }.into_any()
        }
        SemType::Bool => {
            let yes = v.as_bool().unwrap_or(false);
            view! { <span class="font-medium" data-value=yes.to_string()>{if yes { "Yes" } else { "No" }}</span> }.into_any()
        }
        SemType::Enum(c) => {
            let raw = plain(v);
            let opt = c.options.iter().find(|o| o.value == raw);
            let label = opt.map(|o| o.label.clone()).unwrap_or(raw.clone());
            let tone = if opt.is_some_and(|o| o.danger) {
                Tone::Danger
            } else {
                Tone::Neutral
            };
            view! { <Badge text=label tone=tone title=raw/> }.into_any()
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
        SemType::Ref(c) => ref_link(c.entity.section(), &plain(v), ctx),
        SemType::RefList(c) => {
            let section = c.entity.section().to_string();
            let items = names(v);
            if items.is_empty() {
                return view! { <span class="text-text-muted">"None"</span> }.into_any();
            }
            view! {
                <span class="flex flex-wrap gap-1">
                    {items.iter().map(|n| ref_link(&section, n, ctx)).collect_view()}
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
                            <th scope="row">{ref_link(&section, k, ctx)}</th>
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
