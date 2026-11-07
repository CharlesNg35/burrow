use burrow_admin_logic::tree::inner;
use burrow_admin_proto::Path;
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
                            <button type="button" class=button_class(burrow_ui_kit::ButtonLook::Ghost, burrow_ui_kit::ButtonSize::Small) aria-label=format!("Remove {} {}", fc.node.label, i + 1) on:click=remove>
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
            <button type="button" id=format!("{}-add-button", fc.id) class=button_class(burrow_ui_kit::ButtonLook::Secondary, burrow_ui_kit::ButtonSize::Small) on:click=add>
                {burrow_admin_logic::wording::add_label(&fc.node.label, &item_label)}
            </button>
        </div>
    }
    .into_any()
}

pub(crate) fn object_list_card(node: Node, item: Node, path: Path) -> AnyView {
    let s = store();
    let e = s.edit;
    let editing = crate::form::editing_now();
    let locked = s
        .schema
        .get_untracked()
        .is_some_and(|sc| sc.locked_paths().iter().any(|l| path.starts_with(l)));
    let can = s.can_write_now() && node.editable.is_yes() && !locked;
    let p = path.clone();
    let count = Memo::new(move |_| match e.value(&p) {
        Some(Value::Array(a)) => a.len(),
        _ => 0,
    });
    let entry = inner(&item).clone();
    let item_label = item.label.clone();
    let list_label = node.label.clone();
    let id = crate::form::field_id(&path);
    let rows = {
        let path = path.clone();
        let item_label = item_label.clone();
        move || {
            let writing = can && editing.try_get().unwrap_or(false);
            (0..count.get())
                .map(|i| {
                    let p = child(&path, &i.to_string());
                    let sp = child(&path, "*");
                    let list = path.clone();
                    let remove = move |_| {
                        if let Some(Value::Array(mut a)) = e.value(&list) {
                            a.remove(i);
                            e.set(&list, (!a.is_empty()).then_some(Value::Array(a)));
                        }
                    };
                    let title = format!("{} {}", item_label, i + 1);
                    view! {
                        <div class="mt-3 rounded-[10px] border border-border px-3.5 dark:bg-surface-2" data-item=i.to_string()>
                            <div class="flex items-center justify-between gap-2 border-b border-rowline py-2">
                                <span class="text-[13px] font-semibold">{title.clone()}</span>
                                {writing.then(|| view! {
                                    <button type="button" class=button_class(burrow_ui_kit::ButtonLook::Ghost, burrow_ui_kit::ButtonSize::Small) aria-label=format!("Remove {title}") data-testid="remove-item" on:click=remove>
                                        "Remove"
                                    </button>
                                })}
                            </div>
                            {object_view(&entry, p, sp, Vec::new())}
                        </div>
                    }
                })
                .collect_view()
        }
    };
    let add = {
        let list = path.clone();
        move |_| {
            let mut a = match e.value(&list) {
                Some(Value::Array(a)) => a,
                _ => Vec::new(),
            };
            a.push(Value::Object(Map::new()));
            e.set(&list, Some(Value::Array(a)));
        }
    };
    let empty = node.empty_label.clone().unwrap_or_else(|| "None".into());
    let add_id = format!("{id}-add-button");
    view! {
        <div id=id.clone() data-list-card="">
            {move || (count.get() == 0).then(|| view! { <p class="py-2 text-sm text-text-muted" data-testid="list-empty">{empty.clone()}</p> })}
            {rows}
            {move || (can && editing.try_get().unwrap_or(false)).then(|| view! {
                <button type="button" id=add_id.clone() class=format!("mt-3 {}", button_class(burrow_ui_kit::ButtonLook::Secondary, burrow_ui_kit::ButtonSize::Small)) on:click=add.clone()>
                    {burrow_admin_logic::wording::add_label(&list_label, &item_label)}
                </button>
            })}
        </div>
    }
    .into_any()
}
