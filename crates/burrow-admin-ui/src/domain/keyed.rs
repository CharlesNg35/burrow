use burrow_admin_logic::check::check;
use burrow_admin_proto::schema::{Entity, Node, SemType};
use burrow_admin_proto::serde_json::Value;

use crate::store::store;

use super::FieldCtx;
use burrow_admin_logic::shorthand::Shorthand;
use burrow_ui_kit::{TextInput, Toggle, button_class};
use leptos::prelude::*;

fn example(key: &Node, label: &str) -> &'static str {
    match key.sem {
        SemType::UrlPath => "/old",
        SemType::HostName(_) => "old.example.com",
        SemType::Name(ref c) if c.entity == Entity::Login => "authelia",
        _ if label.contains("header") => "X-Frame-Options",
        _ => "old.example.com",
    }
}

pub(super) fn key_adder(
    id: String,
    label: String,
    key: Node,
    used: Memo<Vec<String>>,
    add: impl Fn(String) + Clone + Send + Sync + 'static,
) -> AnyView {
    let text = RwSignal::new(String::new());
    let k2 = key.clone();
    let problem = Memo::new(move |_| {
        let t = text.get().trim().to_string();
        if t.is_empty() {
            return None;
        }
        if used.get().contains(&t) {
            return Some(format!("{t} is already set"));
        }
        match &k2.sem {
            SemType::Text(_) => None,
            _ => check(&k2, &Value::String(t)).err(),
        }
    });
    let msg_id = format!("{id}-add-msg");
    let commit = move || {
        let t = text.get_untracked().trim().to_string();
        if !t.is_empty() && problem.get_untracked().is_none() {
            add(t);
            text.set(String::new());
        }
    };
    let c2 = commit.clone();
    let button_id = format!("{id}-add-button");
    view! {
        <div class="flex items-start gap-2 pt-1">
            <div class="min-w-0 flex-1">
                <TextInput
                    id=format!("{id}-add")
                    value=text
                    on_input=move |v: String| text.set(v)
                    placeholder=format!("for example {}", example(&key, &label.to_lowercase()))
                    label=format!("Add to {label}")
                    invalid=Signal::derive(move || problem.get().is_some())
                    described_by=Signal::derive({
                        let m = msg_id.clone();
                        move || if problem.get().is_some() { m.clone() } else { String::new() }
                    })
                    mono=true
                />
                {move || problem.get().map(|p| view! { <p id=format!("{id}-add-msg") class="mt-1 text-xs text-danger">{p}</p> })}
            </div>
            <button type="button" id=button_id class=button_class("secondary", true) on:click=move |_| c2()>
                "Add"
            </button>
        </div>
    }
    .into_any()
}

pub(super) fn string_editor(fc: FieldCtx) -> AnyView {
    let e = store().edit;
    let path = fc.path.clone();
    let value = Signal::derive(move || {
        e.value(&path)
            .as_ref()
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    });
    let p2 = fc.path.clone();
    let touched = fc.touched;
    view! {
        <TextInput
            id=fc.id.clone()
            value=value
            on_input=move |t: String| {
                touched.set(true);
                e.set(&p2, Some(Value::String(t)));
            }
            placeholder="empty: remove the header"
            label=fc.node.label.clone()
        />
    }
    .into_any()
}

pub(super) fn shorthand_editor(fc: FieldCtx, sh: Shorthand) -> AnyView {
    let e = store().edit;
    let sh = StoredValue::new(sh);
    let path = fc.path.clone();
    let text = Signal::derive(move || sh.with_value(|s| s.text_of(e.value(&path).as_ref())));
    let p_text = fc.path.clone();
    let touched = fc.touched;
    let flags = sh.with_value(|s| s.flags.clone());
    let toggles = flags
        .into_iter()
        .map(|f| {
            let key = f.key.clone();
            let (p_read, p_write) = (fc.path.clone(), fc.path.clone());
            let k2 = key.clone();
            let on = Signal::derive(move || {
                sh.with_value(|s| s.flag_of(e.value(&p_read).as_ref(), &k2))
            });
            let label = f.node.label.clone();
            view! {
                <Toggle
                    id=format!("{}-{key}", fc.id)
                    checked=on
                    label=label
                    on_change=move |v: bool| {
                        touched.set(true);
                        let next = sh.with_value(|s| s.set_flag(e.value(&p_write).as_ref(), &key, v));
                        e.set(&p_write, Some(next));
                    }
                />
            }
        })
        .collect_view();
    view! {
        <div class="flex flex-wrap items-center gap-3" id=fc.id.clone() data-editor="shorthand">
            <div class="min-w-0 flex-1">
                <TextInput
                    id=format!("{}-text", fc.id)
                    value=text
                    on_input=move |t: String| {
                        touched.set(true);
                        let next = sh.with_value(|s| s.set_text(e.value(&p_text).as_ref(), &t));
                        e.set(&p_text, Some(next));
                    }
                    placeholder="https://example.com"
                    label=sh.with_value(|s| s.text.node.label.clone())
                    mono=true
                />
            </div>
            {toggles}
        </div>
    }
    .into_any()
}
