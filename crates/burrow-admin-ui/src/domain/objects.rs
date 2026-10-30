use burrow_admin_logic::tree::inner;
use burrow_admin_proto::schema::Node;
use burrow_admin_proto::serde_json::{Map, Value};
use burrow_admin_proto::value::child;
use burrow_ui_kit::button_class;
use leptos::prelude::*;

use crate::form::object_view;
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
                            {object_view(&node, p, sp, fc.entity.clone())}
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
                {format!("Add {}", if item_label.is_empty() { "one".to_string() } else { item_label.to_lowercase() })}
            </button>
        </div>
    }
    .into_any()
}
