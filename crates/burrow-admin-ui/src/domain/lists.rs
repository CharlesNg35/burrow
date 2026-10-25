use burrow_admin_logic::check::check;
use burrow_admin_logic::input::{parse_input, to_text};
use burrow_admin_logic::tree::inner;
use burrow_admin_proto::schema::{Entity, Node, SemType, UnionC};
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::value::child;
use burrow_config::ServiceEntry;
use burrow_ui_kit::{ChipInput, Segmented, Select, button_class};
use leptos::prelude::*;

use crate::form::object_view;
use crate::store::store;

use super::basic::text_editor;
use super::{FieldCtx, THIS_SERVER, cidr_size, editor, names_of, set_checked};

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

pub(super) fn list_editor(fc: FieldCtx, item: Node) -> AnyView {
    if matches!(inner(&item).sem, SemType::Object(_)) {
        return super::objects::object_list(fc, item);
    }
    let e = store().edit;
    let path = fc.path.clone();
    let items = Signal::derive(move || match e.value(&path) {
        Some(Value::Array(a)) => a.iter().map(|x| to_text(Some(x))).collect(),
        Some(Value::String(s)) if s != ANY => vec![s],
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
        if crate::grants::refuses(&fc2.path, v.as_ref()) {
            store().edit.refuse(&fc2.path, true);
            return;
        }
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
    }
    .into_any()
}

pub(super) fn map_editor(
    fc: FieldCtx,
    entity: Option<Entity>,
    key: Option<Node>,
    value_node: Node,
) -> AnyView {
    let e = store().edit;
    let path = fc.path.clone();
    let keys = Memo::new(move |_| match e.value(&path) {
        Some(Value::Object(m)) => m.keys().cloned().collect::<Vec<_>>(),
        _ => Vec::new(),
    });
    let adding = RwSignal::new(String::new());
    let fc2 = fc.clone();
    let ent = entity.clone();
    let reserved = fc.node.sem.reserved().to_vec();
    let candidates = Signal::derive(move || {
        let used = keys.get();
        let mut o = vec![(String::new(), "Add…".to_string())];
        let self_ready = burrow_config::ServerConfig::SELF
            .then(burrow_config::SelfEntry::ALLOW)
            .get(&e.doc())
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty());
        o.extend(
            reserved
                .iter()
                .filter(|n| !used.contains(n))
                .filter(|n| n.as_str() != burrow_config::SELF || self_ready)
                .map(|n| (n.clone(), format!("{THIS_SERVER} · {n}"))),
        );
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
    let free = key.is_some() && entity.is_none();
    let add_default = match &inner(&value_node).sem {
        _ if free => Value::String(String::new()),
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
                let p = child(&fc2.path, &k);
                let sp = child(&fc2.spath, "*");
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
                    <div class="grid grid-cols-[minmax(0,8rem)_minmax(0,1fr)_auto] items-start gap-2 py-1" data-key=k.clone()>
                        <span class="truncate pt-2 font-mono text-sm">{label}</span>
                        <div class="min-w-0">{if free && matches!(inner(&value_node).sem, SemType::Text(_)) { super::keyed::string_editor(sub) } else { editor(sub) }}</div>
                        <button
                            type="button"
                            class=button_class("ghost", true)
                            aria-label=format!("Remove {k}")
                            on:click=move |_| e.remove_pruned(&p)
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
            {key.filter(|_| entity.is_none()).map(|key| super::keyed::key_adder(fc.id.clone(), fc.node.label.clone(), key, keys, add.clone()))}
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

fn any_or_rules(u: &UnionC) -> Option<Node> {
    let any = u.branches.iter().any(
        |b| matches!(&inner(b).sem, SemType::Enum(c) if c.options.iter().any(|o| o.value == ANY)),
    );
    let list = u.branches.iter().find_map(|b| match &inner(b).sem {
        SemType::List(c) => Some((*c.item).clone()),
        _ => None,
    });
    list.filter(|_| any)
}

const ANY: &str = "any";

fn any_or_rules_editor(fc: FieldCtx, item: Node) -> AnyView {
    let e = store().edit;
    let path = fc.path.clone();
    let mode = Memo::new(move |_| match e.value(&path) {
        Some(Value::String(s)) if s == ANY => ANY.to_string(),
        _ => "only".to_string(),
    });
    let p2 = fc.path.clone();
    let kept: StoredValue<Option<Value>> = StoredValue::new(None);
    let options = vec![
        (ANY.to_string(), "Any target".to_string(), false),
        ("only".to_string(), "Only these".to_string(), false),
    ];
    let chips = {
        let fc = fc.clone();
        move || {
            (mode.get() == "only").then(|| {
                view! { <div class="mt-2">{list_editor(fc.clone(), item.clone())}</div> }
            })
        }
    };
    view! {
        <div id=fc.id.clone()>
            <Segmented
                name=format!("{}-mode", fc.id)
                options=options
                value=mode
                label=fc.node.label.clone()
                on_change=move |m: String| {
                    let v = if m == ANY {
                        kept.set_value(e.peek(&p2).filter(|v| v.as_str() != Some(ANY)));
                        Value::String(ANY.into())
                    } else {
                        kept.get_value().unwrap_or(Value::Array(Vec::new()))
                    };
                    e.set(&p2, Some(v));
                }
            />
            {move || (mode.get() == ANY).then(|| view! {
                <p class="mt-1.5 text-xs text-text-muted">"Whatever the client's own allow list permits."</p>
            })}
            {chips}
        </div>
    }
    .into_any()
}

pub(super) fn union_editor(fc: FieldCtx, u: UnionC) -> AnyView {
    if let Some(item) = any_or_rules(&u) {
        return any_or_rules_editor(fc, item);
    }
    if super::website::is_site_tls(&fc) {
        return super::website::tls_editor(fc);
    }
    if super::target_tls::is_target_tls(&fc) {
        return super::target_tls::target_tls_editor(fc);
    }
    if super::website::host_item(&u).is_some() {
        return super::website::website_editor(fc);
    }
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
    let split = ServiceEntry::HEALTH_CHECK.is(&fc.spath);
    let path = fc.path.clone();
    let mode = Memo::new(move |_| match e.value(&path) {
        None | Some(Value::Bool(false)) => "off".to_string(),
        Some(Value::Bool(true)) => "on".to_string(),
        Some(_) => "custom".to_string(),
    });
    let p2 = fc.path.clone();
    let options = vec![
        ("off".to_string(), "Off".to_string(), false),
        ("on".to_string(), "On".to_string(), false),
        ("custom".to_string(), "Custom".to_string(), false),
    ];
    let custom = {
        let (path, spath, entity) = (fc.path.clone(), fc.spath.clone(), fc.entity.clone());
        move || {
            (mode.get() == "custom")
                .then(|| object_view(&obj, path.clone(), spath.clone(), entity.clone()))
        }
    };
    let on_change = Callback::new(move |m: String| {
        let v = match m.as_str() {
            "on" => Some(Value::Bool(true)),
            "custom" => Some(Value::Object(Default::default())),
            _ => None,
        };
        e.set(&p2, v);
    });
    let below = (!split).then(|| view! {
        {move || (mode.get() == "custom").then(|| view! {
            <div class="mt-3 rounded-[10px] border border-border bg-surface px-3.5 py-3 md:[&_section]:grid-cols-3 dark:bg-surface-2" data-testid="custom-box">
                {custom.clone()}
            </div>
        })}
    });
    let choice = super::Choice {
        options,
        mode: mode.into(),
        on_change,
    };
    super::choice_box(&fc, None, choice, below)
}
