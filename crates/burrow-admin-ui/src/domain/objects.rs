use burrow_admin_logic::tree::{inner, kinds};
use burrow_admin_proto::Path;
use burrow_admin_proto::schema::{Node, SemType};
use burrow_admin_proto::serde_json::{Map, Value};
use burrow_admin_proto::value::child;
use burrow_ui_kit::{Segmented, button_class};
use leptos::prelude::*;

use crate::form::{object_view, object_view_skipping};
use crate::store::store;

use super::FieldCtx;

pub(super) fn object_list(fc: FieldCtx, item: Node) -> AnyView {
    let e = store().edit;
    let path = fc.path.clone();
    let count = Memo::new(move |_| match e.value(&path) {
        Some(Value::Array(a)) => a.len(),
        _ => 0,
    });
    let node = inner(&item).clone();
    let item_label = item.label.clone();
    let rows = {
        let fc = fc.clone();
        move || {
            (0..count.get())
                .map(|i| {
                    let p = child(&fc.path, &i.to_string());
                    let sp = child(&fc.spath, "*");
                    let remove = {
                        let list = fc.path.clone();
                        move |_| {
                            if let Some(Value::Array(mut a)) = e.value(&list) {
                                a.remove(i);
                                e.set(&list, (!a.is_empty()).then_some(Value::Array(a)));
                            }
                        }
                    };
                    view! {
                        <div class="rounded-input border border-border px-3 py-2" data-item=i.to_string()>
                            {item_view(&node, p, sp, fc.entity.clone())}
                            <button type="button" class=button_class("ghost", true) aria-label=format!("Remove {} {}", fc.node.label, i + 1) on:click=remove>
                                "Remove"
                            </button>
                        </div>
                    }
                })
                .collect_view()
        }
    };
    let add = {
        let list = fc.path.clone();
        move |_| {
            let mut a = match e.value(&list) {
                Some(Value::Array(a)) => a,
                _ => Vec::new(),
            };
            a.push(Value::Object(Map::new()));
            e.set(&list, Some(Value::Array(a)));
        }
    };
    view! {
        <div class="space-y-2" id=fc.id.clone()>
            {rows}
            <button type="button" id=format!("{}-add-button", fc.id) class=button_class("secondary", true) on:click=add>
                {burrow_admin_logic::wording::add_label(&fc.node.label, &item_label)}
            </button>
        </div>
    }
    .into_any()
}

fn item_view(node: &Node, path: Path, spath: Path, entity: Path) -> AnyView {
    let all: Vec<(String, String, bool)> = kinds(node)
        .iter()
        .map(|f| {
            (
                f.key.clone(),
                f.node.label.clone(),
                matches!(inner(&f.node).sem, SemType::Object(_)),
            )
        })
        .collect();
    if all.is_empty() {
        return object_view(node, path, spath, entity);
    }
    let e = store().edit;
    let set = {
        let (path, all) = (path.clone(), all.clone());
        move || {
            all.iter()
                .find(|(k, _, _)| e.value(&child(&path, k)).is_some_and(|v| !v.is_null()))
                .map(|(k, _, _)| k.clone())
                .unwrap_or_default()
        }
    };
    let picked = RwSignal::new(untrack(set));
    let on_change = {
        let (path, all) = (path.clone(), all.clone());
        Callback::new(move |k: String| {
            picked.set(k.clone());
            let mut m = match e.value(&path) {
                Some(Value::Object(m)) => m,
                _ => Map::new(),
            };
            for (key, _, _) in &all {
                m.remove(key);
            }
            if all.iter().any(|(key, _, object)| *key == k && *object) {
                m.insert(k, Value::Object(Map::new()));
            }
            e.set(&path, Some(Value::Object(m)));
        })
    };
    let options: Vec<(String, String, bool)> = all
        .iter()
        .map(|(k, label, _)| (k.clone(), label.clone(), false))
        .collect();
    let node = node.clone();
    let name = format!("{}-kind", path.join("-"));
    let fields = move || {
        let now = picked.get();
        let skip: Vec<&str> = all
            .iter()
            .map(|(k, _, _)| k.as_str())
            .filter(|k| *k != now)
            .collect();
        object_view_skipping(&node, path.clone(), spath.clone(), entity.clone(), &skip)
    };
    view! {
        <div class="mb-2" data-testid="kind-choice">
            <Segmented name=name options=options value=picked on_change=on_change label=burrow_admin_logic::wording::KIND/>
        </div>
        {fields}
    }
    .into_any()
}
