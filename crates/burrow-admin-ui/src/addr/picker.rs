use burrow_admin_logic::choices::{Presence, ServiceChoice, client_choices, service_choices};
use burrow_admin_proto::schema::Entity;
use burrow_admin_proto::serde_json::Value;
use burrow_config::{ClientEntry, Entry, VisitorEntry};
use burrow_ui_kit::{ComboOption, Combobox, Tone};
use leptos::prelude::*;

use crate::domain::{FieldCtx, names_of, set_checked};
use crate::store::store;

pub(super) fn options_for(
    entity: &Entity,
    field: &[String],
    reserved: &[String],
) -> Vec<ComboOption> {
    if *entity == Entity::Client {
        return client_options(field, reserved);
    }
    if *entity == Entity::Service {
        let status = store().status.get();
        return service_choices(&store().edit.doc(), status.as_deref())
            .into_iter()
            .map(service_option)
            .collect();
    }
    names_of(entity)
        .into_iter()
        .map(|n| match entity {
            Entity::Visitor => visitor_option(n),
            _ => ComboOption {
                value: n.clone(),
                label: n,
                ..ComboOption::default()
            },
        })
        .collect()
}

fn client_options(field: &[String], reserved: &[String]) -> Vec<ComboOption> {
    let status = store().status.get();
    let doc = store().edit.doc();
    client_choices(&doc, status.as_deref(), reserved, field)
        .into_iter()
        .map(|c| ComboOption {
            dot: match c.presence {
                _ if c.blocked.is_some() => Some(Tone::Neutral),
                Presence::Here | Presence::Online => Some(Tone::Success),
                Presence::Offline => Some(Tone::Neutral),
                Presence::Unknown => None,
            },
            value: c.name,
            label: c.label,
            facet: c.detail,
            blocked: c.blocked.map(str::to_string),
            group: u8::from(c.presence != Presence::Here),
        })
        .collect()
}

pub fn service_option(c: ServiceChoice) -> ComboOption {
    ComboOption {
        dot: c.up.map(|u| if u { Tone::Success } else { Tone::Neutral }),
        value: c.name.clone(),
        label: c.name,
        facet: c.detail,
        ..ComboOption::default()
    }
}

fn visitor_option(n: String) -> ComboOption {
    let open = store().status.with(|st| {
        st.as_ref()
            .and_then(|st| st.visitors.iter().find(|v| v.name == n))
            .map_or(0, |v| v.open)
    });
    let admin = store()
        .edit
        .peek(&VisitorEntry::ADMIN.of(&n))
        .map(|v| crate::form::plain(&v));
    let mut parts: Vec<String> = Vec::new();
    if let Some(a) = admin {
        parts.push(format!("admin · {a}, uses burrow admin"));
    }
    if open > 0 {
        parts.push("in use now".into());
    }
    ComboOption {
        value: n.clone(),
        label: n,
        facet: parts.join(" · "),
        ..ComboOption::default()
    }
}

#[component]
pub fn RefPicker(
    #[prop(into)] id: String,
    entity: Entity,
    field: Vec<String>,
    reserved: Vec<String>,
    #[prop(into)] selected: Signal<Vec<String>>,
    #[prop(into)] on_change: Callback<Vec<String>>,
    #[prop(optional, into)] hidden: Signal<Vec<String>>,
    #[prop(optional)] multi: bool,
    #[prop(optional)] clearable: bool,
    #[prop(optional, into)] header: String,
    #[prop(optional, into)] placeholder: String,
    #[prop(optional, into)] label: String,
    #[prop(optional, into)] invalid: Signal<bool>,
    #[prop(optional, into)] described_by: String,
    #[prop(optional, into)] create_label: String,
    #[prop(default = None)] on_create: OnCreate,
) -> impl IntoView {
    let options = Signal::derive(move || {
        let skip = hidden.get();
        let mut all = options_for(&entity, &field, &reserved);
        all.retain(|o| !skip.contains(&o.value));
        all
    });
    view! {
        <Combobox id=id options=options selected=selected on_change=on_change multi=multi clearable=clearable header=header placeholder=placeholder label=label invalid=invalid described_by=described_by create_label=create_label on_create=on_create/>
    }
}

pub type OnCreate = Option<Callback<String, Option<String>>>;

pub fn create_for(entity: &Entity) -> (String, OnCreate) {
    let section = match entity {
        Entity::Client => Some(ClientEntry::section()),
        Entity::Visitor => Some(VisitorEntry::section()),
        _ => None,
    };
    let can = store().can_write_now();
    let create_label = section
        .map(|s| format!("New {}…", crate::screens::create::singular(s)))
        .unwrap_or_default();
    let on_create = section.filter(|_| can).map(|sec| {
        Callback::new(move |name: String| {
            if let Some(p) = crate::screens::create::name_problem(&name, sec, None) {
                return Some(p);
            }
            crate::screens::create::create_with(sec, &name, None, None, &[]);
            None
        })
    });
    (create_label, on_create)
}

pub fn merged_partner(spath: &[String]) -> Option<String> {
    let (key, parent) = spath.split_last()?;
    store().schema.with_untracked(|s| {
        let obj = s.as_ref()?.root.at(parent)?;
        burrow_admin_logic::tree::merged_single(obj, key).map(str::to_string)
    })
}

fn as_names(v: Option<Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
        Some(Value::String(s)) => vec![s],
        _ => Vec::new(),
    }
}

fn only_one(entity: &Entity) -> Vec<String> {
    let all = names_of(entity);
    if all.len() == 1 {
        all
    } else {
        vec!["true".to_string()]
    }
}

pub fn picker(fc: FieldCtx, entity: Entity, multi: bool) -> AnyView {
    let e = store().edit;
    let path = fc.path.clone();
    let partner = merged_partner(&fc.spath);
    let merged = partner.is_some();
    let single_path = {
        let mut p = fc.path.clone();
        p.pop();
        p.extend(partner);
        p
    };
    let sp = single_path.clone();
    let ent_shown = entity.clone();
    let selected = Signal::derive(move || {
        let mut names = if merged {
            as_names(e.value(&sp))
        } else {
            Vec::new()
        };
        let given = match e.value(&path) {
            Some(Value::Bool(true)) => only_one(&ent_shown),
            v => as_names(v),
        };
        for n in given {
            if !names.contains(&n) {
                names.push(n);
            }
        }
        names
    });
    let header = {
        let s = store();
        let plural = entity.plural();
        let online = if entity == Entity::Client {
            s.status.with_untracked(|st| {
                st.as_ref().map(|st| {
                    st.clients
                        .iter()
                        .filter(|c| c.connected && !c.local)
                        .count()
                })
            })
        } else {
            None
        };
        match online {
            Some(n) => format!("{plural} · {n} online"),
            None => plural.to_string(),
        }
    };
    let node = fc.node.clone();
    let fc2 = fc.clone();
    let on_change = Callback::new(move |list: Vec<String>| {
        if merged {
            let one = list.len() == 1;
            e.set(&single_path, one.then(|| Value::String(list[0].clone())));
            let many = (list.len() > 1)
                .then(|| Value::Array(list.iter().cloned().map(Value::String).collect()));
            set_checked(&fc2, &node, many);
            return;
        }
        let v = if multi {
            (!list.is_empty()).then(|| Value::Array(list.into_iter().map(Value::String).collect()))
        } else {
            list.into_iter().next().map(Value::String)
        };
        set_checked(&fc2, &node, v);
    });
    let touched = fc.touched;
    let invalid = {
        let path = fc.path.clone();
        Signal::derive(move || touched.get() && e.local.with(|l| l.contains_key(&path)))
    };
    let (create_label, on_create) = create_for(&entity);
    view! {
        <div data-picker=if multi { "multi" } else { "single" } on:focusout=move |_| { let _ = touched.try_set(true); }>
            <RefPicker
                id=fc.id.clone()
                entity=entity.clone()
                field=fc.spath.clone()
                reserved=fc.node.sem.reserved().to_vec()
                selected=selected
                on_change=on_change
                multi=multi
                clearable=!fc.node.required
                header=header
                placeholder=if multi { "Add…" } else { "Pick one…" }
                label=fc.node.label.clone()
                invalid=invalid
                described_by=fc.msg_id.clone()
                create_label=create_label
                on_create=on_create
            />
        </div>
    }
    .into_any()
}
