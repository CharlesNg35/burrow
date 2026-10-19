use std::sync::Arc;

use burrow_admin_logic::fallback::{Render, for_node, needs_newer_ui};
use burrow_admin_logic::relations::field_state;
use burrow_admin_logic::tree::{fields, inner};
use burrow_admin_proto::Path;
use burrow_admin_proto::schema::{Applies, DurationUnit, Editable, Field, Node, Schema, SemType};
use burrow_admin_proto::serde_json::{self, Value};
use burrow_admin_proto::value::{SecretSrc, as_env, as_secret};
use burrow_syntax::duration::{TimeUnit, human};
use burrow_ui_kit::{Badge, Chip, FieldRow, FieldState, Tone, chip_class};
use leptos::prelude::*;

use crate::route::{Link, Route};

const MAX_TEXT: usize = 4096;

#[derive(Clone)]
pub struct Ctx {
    pub schema: Arc<Schema>,
    pub doc: Value,
}

fn join(p: &[String]) -> String {
    p.join("/")
}

fn push(p: &[String], k: &str) -> Path {
    let mut out = p.to_vec();
    out.push(k.to_string());
    out
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

fn applies_badge(n: &Node) -> Option<(String, &'static str)> {
    match n.applies {
        Some(Applies::Live) => Some(("Applies live".into(), "success")),
        Some(Applies::Reload) => Some(("On reload".into(), "info")),
        Some(Applies::Restart) => Some(("After restart".into(), "warning")),
        Some(Applies::Unknown) | None => None,
    }
}

fn locked(n: &Node) -> Option<String> {
    match &n.editable {
        Editable::Yes => None,
        Editable::No { reason } if reason.is_empty() => Some("Not editable here".into()),
        Editable::No { reason } => Some(reason.clone()),
    }
}

pub fn object_view(
    node: &Node,
    value: Option<&Value>,
    path: Path,
    spath: Path,
    entity: Path,
    ctx: &Ctx,
) -> AnyView {
    object_view_skipping(node, value, path, spath, entity, ctx, &[])
}

pub fn object_view_skipping(
    node: &Node,
    value: Option<&Value>,
    path: Path,
    spath: Path,
    entity: Path,
    ctx: &Ctx,
    skip: &[&str],
) -> AnyView {
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
    let sections = groups
        .into_iter()
        .filter_map(|(key, label)| {
            let rows: Vec<AnyView> = all
                .iter()
                .filter(|f| f.node.group == key)
                .map(|f| {
                    let v = value.and_then(|v| v.get(&f.key));
                    row(
                        f,
                        v,
                        push(&path, &f.key),
                        push(&spath, &f.key),
                        &entity,
                        ctx,
                    )
                })
                .collect();
            (!rows.is_empty()).then(|| {
                let title =
                    (!label.is_empty()).then(|| view! { <h3 class="mt-2 mb-1 text-sm font-semibold tracking-wide text-text-muted uppercase">{label}</h3> });
                view! {
                    <section class="mb-4 divide-y divide-border" data-group=key>
                        {title}
                        {rows}
                    </section>
                }
            })
        })
        .collect_view();
    view! { <div class="max-w-5xl">{sections}</div> }.into_any()
}

fn row(f: &Field, v: Option<&Value>, path: Path, spath: Path, entity: &Path, ctx: &Ctx) -> AnyView {
    let st = field_state(&f.node, &ctx.schema.root, &ctx.doc, entity);
    if !st.visible {
        return ().into_any();
    }
    let n = inner(&f.node);
    if let SemType::Object(_) = n.sem
        && !needs_newer_ui(n)
    {
        let set = v.is_some_and(|v| !v.is_null());
        let label = f.node.label.clone();
        let body = if set || !matches!(f.node.sem, SemType::Optional(_)) {
            let child_entity = if entity.is_empty() {
                Vec::new()
            } else {
                entity.clone()
            };
            object_view(n, v, path.clone(), spath.clone(), child_entity, ctx)
        } else {
            view! { <p class="text-text-muted" data-testid="not-set">{f.node.none_label.clone().unwrap_or_else(|| "Not set".into())}</p> }.into_any()
        };
        return view! {
            <fieldset class="my-2 rounded-card border border-border p-3" data-path=join(&path) data-schema=join(&spath)>
                <legend class="px-1 font-medium">{label}</legend>
                <p class="mb-2 text-sm text-text-muted">{f.node.help.clone()}</p>
                {body}
            </fieldset>
        }
        .into_any();
    }
    let mut badges: Vec<(String, &'static str)> = applies_badge(&f.node).into_iter().collect();
    if v.and_then(as_env).is_some() {
        badges.push(("From environment".into(), "info"));
    }
    let state = FieldState {
        disabled_reason: (!st.enabled).then(|| st.disabled_reason.clone()).flatten(),
        warning: st.warnings.first().cloned(),
        locked_reason: locked(&f.node),
        ..FieldState::default()
    };
    let id = format!("f-{}", join(&path).replace(['/', ' ', '.'], "-"));
    let shown = value_view(&f.node, v, st.default_from.as_ref(), ctx);
    view! {
        <FieldRow
            id=id.clone()
            label=f.node.label.clone()
            help=f.node.help.clone()
            path=join(&path)
            schema_path=join(&spath)
            badges=badges
            state=Signal::stored(state)
        >
            <div class="min-h-6" id=id>{shown}</div>
        </FieldRow>
    }
    .into_any()
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
        SemType::Object(_) => object_view(node, Some(v), Vec::new(), Vec::new(), Vec::new(), ctx),
        SemType::Unknown(_) => mono(v),
    }
}
