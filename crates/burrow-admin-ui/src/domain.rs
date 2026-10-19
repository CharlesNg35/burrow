use burrow_admin_logic::check::{check, refs_exist};
use burrow_admin_logic::fallback::{Render, for_node};
use burrow_admin_logic::input::{parse_input, rate_parts, rate_value, time_unit, to_text};
use burrow_admin_logic::tree::{inner, names};
use burrow_admin_proto::Path;
use burrow_admin_proto::schema::{Entity, EnumC, Node, SemType, UnionC};
use burrow_admin_proto::serde_json::{self, Value};
use burrow_syntax::duration::{TimeUnit, best_unit, human, to_stored, units_for};
use burrow_syntax::rate::RateUnit;
use burrow_ui_kit::{ChipInput, Segmented, Select, TextArea, TextInput, Toggle, button_class};
use leptos::prelude::*;

use crate::form::{object_view, push};
use crate::store::store;

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

fn names_of(entity: &Entity) -> Vec<String> {
    let doc = store().edit.doc();
    names(&doc, &[entity.section().to_string()])
}

fn set_checked(fc: &FieldCtx, node: &Node, v: Option<Value>) {
    let e = store().edit;
    let problem = match &v {
        None if node.required && node.default.is_none() => {
            Some(format!("{} is required", node.label))
        }
        None => None,
        Some(x) => check(node, x)
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
        Render::Known(sem) | Render::Fallback(sem) => {
            let sem = sem.clone();
            by_sem(fc, &sem)
        }
    }
}

fn by_sem(fc: FieldCtx, sem: &SemType) -> AnyView {
    match sem {
        SemType::Text(_)
        | SemType::Host
        | SemType::ServerAddress
        | SemType::Path(_)
        | SemType::UrlPath
        | SemType::Name(_)
        | SemType::Ip(_)
        | SemType::Port
        | SemType::IpPort(_)
        | SemType::PortRange(_)
        | SemType::Target(_)
        | SemType::Cidr
        | SemType::CidrRule
        | SemType::Integer(_)
        | SemType::Size(_) => text_editor(fc, Some(sem.clone())),
        SemType::Bool => bool_editor(fc),
        SemType::Enum(c) => enum_editor(fc, c.clone()),
        SemType::Duration(c) => duration_editor(fc, time_unit(c.unit), c.max),
        SemType::Rate => rate_editor(fc),
        SemType::Ref(r) => ref_editor(fc, r.entity.clone()),
        SemType::RefList(r) => ref_list_editor(fc, r.entity.clone()),
        SemType::List(l) => list_editor(fc, (*l.item).clone()),
        SemType::RefMap(m) => map_editor(fc, Some(m.entity.clone()), (*m.value).clone()),
        SemType::Map(m) => map_editor(fc, None, (*m.value).clone()),
        SemType::SecretRef(c) => crate::secrets::secret_list(fc.path.clone(), fc.id.clone(), c.max),
        SemType::Union(u) => union_editor(fc, u.clone()),
        SemType::Optional(o) => {
            let inner_node = (*o.inner).clone();
            let fc2 = FieldCtx {
                node: Node {
                    required: false,
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
        .map(|d| format!("Default: {}", to_text(Some(d))))
        .or_else(|| fc.node.none_label.clone())
        .unwrap_or_default()
}

fn text_editor(fc: FieldCtx, sem: Option<SemType>) -> AnyView {
    let e = store().edit;
    let path = fc.path.clone();
    let value = Signal::derive(move || to_text(e.value(&path).as_ref()));
    let node = fc.node.clone();
    let numeric = matches!(
        sem,
        Some(SemType::Integer(_) | SemType::Size(_) | SemType::Port)
    );
    let mono = !matches!(sem, Some(SemType::Text(_)) | None);
    let invalid = {
        let path = fc.path.clone();
        let touched = fc.touched;
        Signal::derive(move || touched.get() && e.local.with(|l| l.contains_key(&path)))
    };
    let fc2 = fc.clone();
    let on_input = move |text: String| {
        let v = match &sem {
            Some(s) => parse_input(s, &text),
            None => (!text.trim().is_empty()).then(|| Value::String(text.trim().to_string())),
        };
        if sem.is_some() {
            set_checked(&fc2, &node, v);
        } else {
            e.local_error(&fc2.path, None);
            e.set(&fc2.path, v);
        }
    };
    let touched = fc.touched;
    let unit = match &fc.node.sem {
        SemType::Integer(c) => c.unit.clone(),
        SemType::Size(c) => Some(c.unit.clone()),
        SemType::Optional(o) => match &o.inner.sem {
            SemType::Integer(c) => c.unit.clone(),
            SemType::Size(c) => Some(c.unit.clone()),
            _ => None,
        },
        _ => None,
    };
    let server_checked = matches!(
        for_node(&fc.node, None),
        Render::ServerChecked | Render::Fallback(_)
    );
    view! {
        <div class="flex items-center gap-2">
            <TextInput
                id=fc.id.clone()
                value=value
                on_input=on_input
                on_blur=move |_| { let _ = touched.try_set(true); }
                placeholder=placeholder(&fc)
                invalid=invalid
                described_by=fc.msg_id.clone()
                numeric=numeric
                mono=mono
            />
            {unit.map(|u| view! { <span class="text-sm text-text-muted">{u}</span> })}
            {server_checked.then(|| view! { <span class="text-xs whitespace-nowrap text-info" data-render="server-checked">"Checked by the server"</span> })}
        </div>
    }
    .into_any()
}

fn yaml_editor(fc: FieldCtx) -> AnyView {
    let e = store().edit;
    let path = fc.path.clone();
    let initial = e
        .peek(&path)
        .map(|v| serde_json::to_string_pretty(&v).unwrap_or_default())
        .unwrap_or_default();
    let p2 = path.clone();
    let value = Signal::derive(move || {
        e.texts
            .with(|t| t.get(&p2).cloned())
            .unwrap_or_else(|| initial.clone())
    });
    view! {
        <div class="rounded-card border border-dashed border-warning p-2" data-testid="raw-node">
            <p class="mb-1 text-sm text-warning">"This setting needs a newer burrow admin to edit it in a form. Edit it as YAML; the server checks it."</p>
            <TextArea id=fc.id.clone() value=value on_input=move |t: String| e.set_text(&path, (!t.trim().is_empty()).then_some(t)) rows=4/>
        </div>
    }
    .into_any()
}

fn bool_editor(fc: FieldCtx) -> AnyView {
    let e = store().edit;
    let path = fc.path.clone();
    let default = fc
        .node
        .default
        .as_ref()
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let checked =
        Signal::derive(move || e.value(&path).and_then(|v| v.as_bool()).unwrap_or(default));
    let p2 = fc.path.clone();
    let danger = fc.node.danger_when_set;
    view! {
        <Toggle
            id=fc.id.clone()
            checked=checked
            label=Signal::derive(move || if checked.get() { "Yes".to_string() } else { "No".to_string() })
            danger=danger
            on_change=move |on: bool| {
                let v = if on == default && !danger { None } else { Some(Value::Bool(on)) };
                e.set(&p2, v)
            }
        />
    }
    .into_any()
}

const UNSET: &str = "\u{1}unset";

fn enum_editor(fc: FieldCtx, c: EnumC) -> AnyView {
    let e = store().edit;
    let path = fc.path.clone();
    let optional = !fc.node.required;
    let default_label = fc
        .node
        .default
        .as_ref()
        .map(|d| {
            let raw = to_text(Some(d));
            let label = c
                .options
                .iter()
                .find(|o| o.value == raw)
                .map(|o| o.label.clone())
                .unwrap_or(raw);
            format!("Default ({label})")
        })
        .unwrap_or_else(|| {
            fc.node
                .none_label
                .clone()
                .unwrap_or_else(|| "Not set".into())
        });
    let value = Signal::derive(move || {
        e.value(&path)
            .map(|v| to_text(Some(&v)))
            .unwrap_or_else(|| UNSET.to_string())
    });
    let p2 = fc.path.clone();
    let set = move |v: String| {
        e.local_error(&p2, None);
        e.set(&p2, (v != UNSET).then(|| Value::String(v)));
    };
    let mut options: Vec<(String, String, bool)> = Vec::new();
    if optional {
        options.push((UNSET.to_string(), default_label, false));
    }
    options.extend(
        c.options
            .iter()
            .map(|o| (o.value.clone(), o.label.clone(), o.danger)),
    );
    let help = {
        let c = c.clone();
        move || {
            let v = value.get();
            c.options
                .iter()
                .find(|o| o.value == v)
                .map(|o| o.help.clone())
                .filter(|h| !h.is_empty())
                .map(|h| view! { <p class="mt-1 text-sm text-text-muted">{h}</p> })
        }
    };
    let control = if options.len() <= 4 {
        view! { <Segmented name=fc.id.clone() options=options value=value on_change=set label=fc.node.label.clone()/> }.into_any()
    } else {
        let opts: Vec<(String, String)> = options.into_iter().map(|(v, l, _)| (v, l)).collect();
        view! { <Select id=fc.id.clone() options=opts value=value on_change=set/> }.into_any()
    };
    view! { <div id=if c.options.len() + usize::from(optional) <= 4 { Some(fc.id.clone()) } else { None }>{control}{help}</div> }.into_any()
}

fn duration_editor(fc: FieldCtx, stored: TimeUnit, max: Option<u64>) -> AnyView {
    let e = store().edit;
    let units = units_for(stored, max);
    let path = fc.path.clone();
    let start = e.peek(&path).and_then(|v| v.as_u64());
    let unit = RwSignal::new(
        start
            .map(|v| best_unit(v, stored, &units))
            .unwrap_or(stored),
    );
    let number = RwSignal::new(
        start
            .map(|v| (v.saturating_mul(stored.ms()) / unit.get_untracked().ms()).to_string())
            .unwrap_or_default(),
    );
    let node = fc.node.clone();
    let fc2 = fc.clone();
    let commit = move || {
        let n = number.get_untracked();
        if n.trim().is_empty() {
            set_checked(&fc2, &node, None);
            return;
        }
        match to_stored(&n, unit.get_untracked(), stored) {
            Ok(v) => set_checked(&fc2, &node, Some(Value::from(v))),
            Err(msg) => {
                e.local_error(&fc2.path, Some(msg));
                e.set(&fc2.path, Some(Value::String(n)));
            }
        }
    };
    let c2 = commit.clone();
    let options: Vec<(String, String)> = units
        .iter()
        .map(|u| (u.short().to_string(), u.label().to_string()))
        .collect();
    let units2 = units.clone();
    let p2 = fc.path.clone();
    let shown = move || {
        e.value(&p2)
            .and_then(|v| v.as_u64())
            .map(|v| format!("= {}", human(v, stored)))
    };
    let touched = fc.touched;
    let invalid = {
        let path = fc.path.clone();
        Signal::derive(move || touched.get() && e.local.with(|l| l.contains_key(&path)))
    };
    view! {
        <div class="flex flex-wrap items-center gap-2">
            <div class="w-32">
                <TextInput
                    id=fc.id.clone()
                    value=number
                    numeric=true
                    invalid=invalid
                    described_by=fc.msg_id.clone()
                    placeholder=placeholder(&fc)
                    on_input=move |t: String| {
                        number.set(t);
                        commit();
                    }
                    on_blur=move |_| { let _ = touched.try_set(true); }
                />
            </div>
            <div class="w-36">
                <Select
                    id=format!("{}-unit", fc.id)
                    label=format!("{} unit", fc.node.label)
                    options=options
                    value=Signal::derive(move || unit.get().short().to_string())
                    on_change=move |u: String| {
                        if let Some(x) = units2.iter().find(|x| x.short() == u) {
                            unit.set(*x);
                            c2();
                        }
                    }
                />
            </div>
            <span class="text-sm text-text-muted">{shown}</span>
        </div>
    }
    .into_any()
}

fn rate_editor(fc: FieldCtx) -> AnyView {
    let e = store().edit;
    let start = e.peek(&fc.path);
    let limited = RwSignal::new(start.as_ref().is_some_and(|v| !v.is_null()));
    let (n0, u0) = rate_parts(start.as_ref());
    let number = RwSignal::new(n0);
    let unit = RwSignal::new(u0);
    let node = fc.node.clone();
    let fc2 = fc.clone();
    let commit = move || {
        set_checked(
            &fc2,
            &node,
            rate_value(&number.get_untracked(), unit.get_untracked()),
        )
    };
    let c2 = commit.clone();
    let c3 = commit.clone();
    let options: Vec<(String, String)> = RateUnit::ALL
        .iter()
        .map(|u| (u.suffix().to_string(), u.label().to_string()))
        .collect();
    let p2 = fc.path.clone();
    let equivalent = move || {
        e.value(&p2)
            .and_then(|v| v.as_str().map(str::to_string))
            .and_then(|t| {
                burrow_syntax::rate::parse_rate(&t)
                    .ok()
                    .map(|b| format!("≈ {} · stored as {t}", burrow_syntax::rate::equivalent(b)))
            })
            .unwrap_or_default()
    };
    let touched = fc.touched;
    let invalid = {
        let path = fc.path.clone();
        Signal::derive(move || touched.get() && e.local.with(|l| l.contains_key(&path)))
    };
    let fc3 = fc.clone();
    let node3 = fc.node.clone();
    let id = fc.id.clone();
    let msg_id = fc.msg_id.clone();
    let label = fc.node.label.clone();
    view! {
        <div class="flex flex-wrap items-center gap-2">
            <Segmented
                name=format!("{}-mode", fc.id)
                label=format!("{} mode", fc.node.label)
                options=vec![("unlimited".to_string(), "Unlimited".to_string(), false), ("limit".to_string(), "Limit".to_string(), false)]
                value=Signal::derive(move || if limited.get() { "limit".to_string() } else { "unlimited".to_string() })
                on_change=Callback::new(move |v: String| {
                    let on = v == "limit";
                    limited.set(on);
                    if on {
                        if number.get_untracked().trim().is_empty() {
                            number.set("10".into());
                        }
                        c3();
                    } else {
                        set_checked(&fc3, &node3, None);
                    }
                })
            />
            <Show when=move || limited.get()>
                <div class="w-[100px]">
                    <TextInput
                        id=id.clone()
                        value=number
                        numeric=true
                        invalid=invalid
                        described_by=msg_id.clone()
                        on_input={
                            let commit = commit.clone();
                            move |t: String| {
                                number.set(t);
                                commit();
                            }
                        }
                        on_blur=move |_| { let _ = touched.try_set(true); }
                    />
                </div>
                <div class="w-32">
                    <Select
                        id=format!("{}-unit", id)
                        label=format!("{label} unit")
                        options=options.clone()
                        value=Signal::derive(move || unit.get().suffix().to_string())
                        on_change={
                            let c2 = c2.clone();
                            move |u: String| {
                                if let Some(x) = RateUnit::from_suffix(&u) {
                                    unit.set(x);
                                    c2();
                                }
                            }
                        }
                    />
                </div>
                <span class="font-mono text-xs text-text-muted">{equivalent.clone()}</span>
            </Show>
        </div>
    }
    .into_any()
}

fn facet(entity: &Entity, name: &str) -> String {
    let s = store();
    match entity {
        Entity::Client => {
            let on = s.status.with(|st| {
                st.as_ref()
                    .and_then(|st| st.clients.iter().find(|c| c.name == name))
                    .is_some_and(|c| c.connected)
            });
            format!("{name} · {}", if on { "online" } else { "offline" })
        }
        _ => name.to_string(),
    }
}

fn ref_editor(fc: FieldCtx, entity: Entity) -> AnyView {
    let e = store().edit;
    let path = fc.path.clone();
    let optional = !fc.node.required;
    let ent = entity.clone();
    let options = Signal::derive(move || {
        let mut o: Vec<(String, String)> = Vec::new();
        if optional {
            o.push((UNSET.to_string(), "Not set".to_string()));
        }
        o.extend(
            names_of(&ent)
                .into_iter()
                .map(|n| (n.clone(), facet(&ent, &n))),
        );
        o
    });
    let value = Signal::derive(move || {
        e.value(&path)
            .map(|v| to_text(Some(&v)))
            .unwrap_or_else(|| UNSET.to_string())
    });
    let node = fc.node.clone();
    let fc2 = fc.clone();
    view! {
        <Select
            id=fc.id.clone()
            options=options
            value=value
            on_change=move |v: String| set_checked(&fc2, &node, (v != UNSET).then(|| Value::String(v)))
        />
    }
    .into_any()
}

fn ref_list_editor(fc: FieldCtx, entity: Entity) -> AnyView {
    let e = store().edit;
    let path = fc.path.clone();
    let current = Signal::derive(move || match e.value(&path) {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
        Some(Value::String(s)) => vec![s],
        _ => Vec::<String>::new(),
    });
    let ent = entity.clone();
    let node = fc.node.clone();
    let fc2 = fc.clone();
    view! {
        <div class="flex flex-wrap gap-2" role="group" aria-label=fc.node.label.clone() id=fc.id.clone()>
            {move || {
                names_of(&ent)
                    .into_iter()
                    .map(|n| {
                        let on = current.with(|c| c.contains(&n));
                        let label = facet(&ent, &n);
                        let n2 = n.clone();
                        let node = node.clone();
                        let fc3 = fc2.clone();
                        view! {
                            <label class="inline-flex cursor-pointer items-center gap-1.5 rounded-input border border-border px-2 py-1 text-sm" data-option=n.clone()>
                                <input
                                    type="checkbox"
                                    class="accent-accent"
                                    prop:checked=on
                                    on:change:target=move |ev| {
                                        let mut list = current.get_untracked();
                                        if ev.target().checked() {
                                            if !list.contains(&n2) {
                                                list.push(n2.clone());
                                            }
                                        } else {
                                            list.retain(|x| x != &n2);
                                        }
                                        let v = (!list.is_empty())
                                            .then(|| Value::Array(list.into_iter().map(Value::String).collect()));
                                        set_checked(&fc3, &node, v);
                                    }
                                />
                                {label}
                            </label>
                        }
                    })
                    .collect_view()
            }}
        </div>
    }
    .into_any()
}

fn list_editor(fc: FieldCtx, item: Node) -> AnyView {
    let e = store().edit;
    let path = fc.path.clone();
    let items = Signal::derive(move || match e.value(&path) {
        Some(Value::Array(a)) => a.iter().map(|x| to_text(Some(x))).collect(),
        _ => Vec::<String>::new(),
    });
    let item2 = item.clone();
    let check_one = Callback::new(move |t: String| {
        let v = parse_input(&item2.sem, &t)?;
        check(&item2, &v).err()
    });
    let node = fc.node.clone();
    let fc2 = fc.clone();
    let item3 = item.clone();
    let on_change = move |list: Vec<String>| {
        let v = (!list.is_empty()).then(|| {
            Value::Array(
                list.iter()
                    .filter_map(|t| parse_input(&item3.sem, t))
                    .collect(),
            )
        });
        set_checked(&fc2, &node, v);
        let bad = list.iter().find_map(|t| {
            parse_input(&item3.sem, t)
                .and_then(|v| check(&item3, &v).err().map(|m| format!("{t}: {m}")))
        });
        if bad.is_some() {
            store().edit.local_error(&fc2.path, bad);
        }
    };
    let cidr = matches!(item.sem, SemType::Cidr | SemType::CidrRule);
    let meta = Callback::new(move |t: String| if cidr { cidr_size(&t) } else { String::new() });
    let note = {
        let path = fc.path.clone();
        move || {
            let last = path.last().map(String::as_str);
            if last != Some("admin_from") {
                return None;
            }
            let mut level_path = path.clone();
            level_path.pop();
            level_path.push("admin".into());
            let write = e
                .value(&level_path)
                .is_some_and(|v| v.as_str() == Some("write"));
            let empty = e
                .value(&path)
                .and_then(|v| v.as_array().map(Vec::is_empty))
                .unwrap_or(true);
            (write && empty).then(|| view! {
                <p class="mt-1.5 flex items-center gap-1.5 text-xs text-info" data-testid="admin-from-note">
                    <burrow_ui_kit::Icon name="info" small=true/>
                    "A write admin without admin_from may administer from any address. Consider limiting it."
                </p>
            })
        }
    };
    view! {
        <ChipInput
            id=fc.id.clone()
            items=items
            on_change=on_change
            check=check_one
            placeholder="Type and press Enter; paste a list"
            empty_label=fc.node.empty_label.clone().unwrap_or_default()
            meta=meta
        />
        {note}
    }
    .into_any()
}

pub fn cidr_size(text: &str) -> String {
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

fn map_editor(fc: FieldCtx, entity: Option<Entity>, value_node: Node) -> AnyView {
    let e = store().edit;
    let path = fc.path.clone();
    let keys = Memo::new(move |_| match e.value(&path) {
        Some(Value::Object(m)) => m.keys().cloned().collect::<Vec<_>>(),
        _ => Vec::new(),
    });
    let adding = RwSignal::new(String::new());
    let fc2 = fc.clone();
    let ent = entity.clone();
    let candidates = Signal::derive(move || {
        let used = keys.get();
        let mut o = vec![(String::new(), "Add…".to_string())];
        if let Some(ent) = &ent {
            o.extend(
                names_of(ent)
                    .into_iter()
                    .filter(|n| !used.contains(n))
                    .map(|n| (n.clone(), facet(ent, &n))),
            );
        }
        o
    });
    let p_add = fc.path.clone();
    let add_default = match &inner(&value_node).sem {
        SemType::Union(_) | SemType::Text(_) | SemType::Enum(_) => Value::String("any".into()),
        SemType::List(_) => Value::Array(Vec::new()),
        _ => Value::Object(Default::default()),
    };
    let add = move |k: String| {
        if k.is_empty() {
            return;
        }
        let mut p = p_add.clone();
        p.push(k);
        e.set(&p, Some(add_default.clone()));
        adding.set(String::new());
    };
    let rows = move || {
        keys.get()
            .into_iter()
            .map(|k| {
                let p = push(&fc2.path, &k);
                let sp = push(&fc2.spath, "*");
                let sub = FieldCtx {
                    node: value_node.clone(),
                    path: p.clone(),
                    spath: sp,
                    entity: fc2.entity.clone(),
                    id: format!(
                        "{}-{}",
                        fc2.id,
                        k.replace(|c: char| !c.is_ascii_alphanumeric(), "-")
                    ),
                    msg_id: fc2.msg_id.clone(),
                    touched: fc2.touched,
                    inherited: None,
                };
                let label = k.clone();
                view! {
                    <div class="flex flex-wrap items-start gap-2 py-1" data-key=k.clone()>
                        <span class="w-32 pt-2 font-mono text-sm">{label}</span>
                        <div class="min-w-0 flex-1">{editor(sub)}</div>
                        <button
                            type="button"
                            class=button_class("ghost", true)
                            aria-label=format!("Remove {k}")
                            on:click=move |_| e.set(&p, None)
                        >
                            "Remove"
                        </button>
                    </div>
                }
            })
            .collect_view()
    };
    view! {
        <div class="space-y-1" id=fc.id.clone()>
            {rows}
            {entity.is_some().then(|| view! {
                <div class="w-60">
                    <Select
                        id=format!("{}-add", fc.id)
                        label=format!("Add to {}", fc.node.label)
                        options=candidates
                        value=adding
                        on_change=add.clone()
                    />
                </div>
            })}
        </div>
    }
    .into_any()
}

fn union_editor(fc: FieldCtx, u: UnionC) -> AnyView {
    let e = store().edit;
    let bool_branch = u
        .branches
        .iter()
        .any(|b| matches!(inner(b).sem, SemType::Bool));
    let object_branch = u
        .branches
        .iter()
        .find(|b| matches!(inner(b).sem, SemType::Object(_)))
        .cloned();
    let Some(obj) = object_branch.filter(|_| bool_branch) else {
        return text_editor(fc, None);
    };
    let path = fc.path.clone();
    let mode = Signal::derive(move || match e.value(&path) {
        None | Some(Value::Bool(false)) => "off".to_string(),
        Some(Value::Bool(true)) => "on".to_string(),
        Some(_) => "custom".to_string(),
    });
    let p2 = fc.path.clone();
    let options = vec![
        ("off".to_string(), "Off".to_string(), false),
        ("on".to_string(), "On, with defaults".to_string(), false),
        ("custom".to_string(), "Custom".to_string(), false),
    ];
    let custom = {
        let (path, spath, entity) = (fc.path.clone(), fc.spath.clone(), fc.entity.clone());
        move || {
            (mode.get() == "custom")
                .then(|| object_view(&obj, path.clone(), spath.clone(), entity.clone()))
        }
    };
    view! {
        <div id=fc.id.clone()>
            <Segmented
                name=fc.id.clone()
                options=options
                value=mode
                label=fc.node.label.clone()
                on_change=move |m: String| {
                    let v = match m.as_str() {
                        "on" => Some(Value::Bool(true)),
                        "custom" => Some(Value::Object(Default::default())),
                        _ => None,
                    };
                    e.set(&p2, v);
                }
            />
            <div class="mt-2">{custom}</div>
        </div>
    }
    .into_any()
}
