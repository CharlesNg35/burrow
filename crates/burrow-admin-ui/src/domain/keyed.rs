use burrow_admin_logic::check::check;
use burrow_admin_proto::schema::{Node, SemType};
use burrow_admin_proto::serde_json::Value;

use crate::store::store;

use super::FieldCtx;
use burrow_ui_kit::{TextInput, button_class};
use leptos::prelude::*;

fn example(key: &Node, label: &str) -> &'static str {
    match key.sem {
        SemType::UrlPath => "/old",
        SemType::HostName(_) => "old.example.com",
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
            <div class="w-60">
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
