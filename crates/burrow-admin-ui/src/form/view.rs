use burrow_admin_logic::fallback::{Render, for_node};
use burrow_admin_logic::tree::{fields, inner};
use burrow_admin_proto::schema::{
    DurationUnit, ListC, MapC, Node, RefListC, RefMapC, SemType, UnionC,
};
use burrow_admin_proto::serde_json::{self, Value};
use burrow_admin_proto::value::{SecretSrc, as_env, as_secret};
use burrow_syntax::duration::{TimeUnit, human};
use burrow_ui_kit::{Badge, Chip, Tone, chip_class};
use leptos::prelude::*;

use crate::route::{Link, Route};

use super::{Ctx, cap, plain};

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

pub(super) fn value_view(
    node: &Node,
    v: Option<&Value>,
    inherited: Option<&Value>,
    ctx: &Ctx,
) -> AnyView {
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
    if burrow_admin_proto::value::is_hash(v) {
        return view! { <span class="text-sm" data-testid="password-status">{burrow_admin_proto::value::display(v)}</span> }.into_any();
    }
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

pub(super) fn mono(v: &Value) -> AnyView {
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
        | SemType::HostName(_)
        | SemType::ServerAddress
        | SemType::Path(_)
        | SemType::Upload(_)
        | SemType::PasswordHash
        | SemType::UrlPath
        | SemType::Name(_) => mono(v),
        SemType::Ip(_)
        | SemType::Port
        | SemType::IpPort(_)
        | SemType::PortRange(_)
        | SemType::Target(_) => mono(v),
        SemType::Integer(c) => {
            let unit = c.unit.clone().map(|u| format!(" {u}")).unwrap_or_default();
            let n = v
                .as_u64()
                .map(crate::format::count)
                .unwrap_or_else(|| plain(v));
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
        SemType::RefList(c) => ref_list_view(c, v, ctx),
        SemType::RefMap(c) => ref_map_view(c, v, ctx),
        SemType::SecretRef(c) if c.reference && v.is_string() => {
            let shown = format!("set, from {}", plain(v));
            view! { <span class="font-mono text-sm" data-secret-ref="">{shown}</span> }.into_any()
        }
        SemType::SecretRef(_) => secrets_view(v),
        SemType::List(c) => list_view(c, v, ctx),
        SemType::Map(c) => map_view(c, v, ctx),
        SemType::Union(u) => union_view(u, v, ctx),
        SemType::Optional(o) => value_view(&o.inner, Some(v), None, ctx),
        SemType::Object(_) => object_static(node, v, ctx),
        SemType::Unknown(_) => mono(v),
    }
}

fn ref_list_view(c: &RefListC, v: &Value, ctx: &Ctx) -> AnyView {
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

fn ref_map_view(c: &RefMapC, v: &Value, ctx: &Ctx) -> AnyView {
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

fn secrets_view(v: &Value) -> AnyView {
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

fn list_view(c: &ListC, v: &Value, ctx: &Ctx) -> AnyView {
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

fn map_view(c: &MapC, v: &Value, ctx: &Ctx) -> AnyView {
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

fn union_view(u: &UnionC, v: &Value, ctx: &Ctx) -> AnyView {
    if u.branches
        .iter()
        .any(|b| matches!(inner(b).sem, SemType::HostName(_)))
        && v.is_object()
    {
        let names = burrow_admin_logic::web::names(v);
        let list = Value::Array(names.into_iter().map(Value::String).collect());
        let branch = u
            .branches
            .iter()
            .find(|b| matches!(inner(b).sem, SemType::List(_)));
        return match branch {
            Some(b) => value_view(b, Some(&list), None, ctx),
            None => mono(&list),
        };
    }
    if u.branches
        .iter()
        .any(|b| matches!(inner(b).sem, SemType::Bool))
        && !matches!(v, Value::String(_))
    {
        return match v {
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
        };
    }
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
