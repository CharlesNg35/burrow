use burrow_admin_proto::schema::Entity;
use burrow_admin_proto::serde_json::Value;
use burrow_config::{ClientEntry, Entry, VisitorEntry};
use burrow_ui_kit::{ComboOption, Combobox, Tone};
use leptos::prelude::*;

use crate::domain::{FieldCtx, names_of, set_checked};
use crate::format;
use crate::store::store;

fn options_for(entity: &Entity, reserved: &[String]) -> Vec<ComboOption> {
    let s = store();
    let status = s.status.get();
    let local = reserved.iter().map(|n| ComboOption {
        value: n.clone(),
        label: crate::domain::THIS_SERVER.to_string(),
        facet: format!("{n} · always connected"),
        dot: Some(Tone::Success),
    });
    local
        .collect::<Vec<_>>()
        .into_iter()
        .chain(names_of(entity).into_iter().map(|n| match entity {
            Entity::Client => {
                let c = status
                    .as_ref()
                    .and_then(|st| st.clients.iter().find(|c| c.name == n))
                    .cloned();
                let on = c.as_ref().is_some_and(|c| c.connected);
                let facet = match &c {
                    Some(c) if c.connected => {
                        let mut f = vec!["online".to_string()];
                        if let Some(x) = c.connected_secs {
                            f.push(format::span(x));
                        }
                        if let Some(from) = &c.from {
                            f.push(from.clone());
                        }
                        f.join(" · ")
                    }
                    Some(c) => format!(
                        "offline · {}",
                        format::left_of(
                            &c.name,
                            c.left_secs_ago,
                            status.as_ref().map(|s| s.uptime_secs).unwrap_or(0)
                        )
                    ),
                    None => String::new(),
                };
                ComboOption {
                    value: n.clone(),
                    label: n,
                    facet,
                    dot: status
                        .is_some()
                        .then_some(if on { Tone::Success } else { Tone::Neutral }),
                }
            }
            Entity::Visitor => {
                let open = status
                    .as_ref()
                    .and_then(|st| st.visitors.iter().find(|v| v.name == n))
                    .map(|v| v.open)
                    .unwrap_or(0);
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
                    dot: None,
                }
            }
            _ => ComboOption {
                value: n.clone(),
                label: n,
                facet: String::new(),
                dot: None,
            },
        }))
        .collect()
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
    let ent = entity.clone();
    let reserved = fc.node.sem.reserved().to_vec();
    let options = Signal::derive(move || options_for(&ent, &reserved));
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
    view! {
        <div data-picker=if multi { "multi" } else { "single" } on:focusout=move |_| { let _ = touched.try_set(true); }>
            <Combobox
                id=fc.id.clone()
                options=options
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
