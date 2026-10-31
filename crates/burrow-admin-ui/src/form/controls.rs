use burrow_admin_proto::Path;
use burrow_admin_proto::schema::Node;
use burrow_admin_proto::serde_json::Value;
use leptos::prelude::*;

use crate::store::store;

use super::field_id;

pub(super) fn object_toggle(
    path: Path,
    is_set: Memo<bool>,
    allowed: bool,
    editing: Signal<bool>,
) -> impl Fn() -> Option<AnyView> + Clone + Send + Sync + 'static {
    let e = store().edit;
    let id = field_id(&path);
    move || {
        (allowed && editing.try_get().unwrap_or(false)).then(|| {
            let (p, id) = (path.clone(), id.clone());
            view! {
                <burrow_ui_kit::Toggle
                    id=id
                    checked=is_set
                    label=Signal::derive(move || if is_set.get() { "On".to_string() } else { "Off".to_string() })
                    on_change=move |on: bool| {
                        e.set(&p, on.then(|| Value::Object(Default::default())));
                    }
                />
            }
            .into_any()
        })
    }
}

pub(crate) const VALUE_BOX: &str = "group relative flex h-[38px] items-center rounded-input border border-border bg-surface-2 px-2.5 font-mono text-[13px] dark:bg-surface-3";

pub(super) fn locked_box(node: Node, path: Path, id: String) -> AnyView {
    let e = store().edit;
    view! {
        <div class=VALUE_BOX id=id data-locked="">
            <span class="flex-1 truncate select-all">
                {move || {
                    let v = e.value(&path);
                    match v.or_else(|| node.default.clone()) {
                        Some(v) => super::view::scalar_text(&node, &v),
                        None => node.none_label.clone().unwrap_or_else(|| super::view::NOT_SET.into()),
                    }
                }}
            </span>
        </div>
    }
    .into_any()
}

pub(super) fn reset_button(path: Path) -> impl IntoView {
    let e = store().edit;
    let modified = {
        let path = path.clone();
        Memo::new(move |_| e.draft.with(|d| d.modified(&path)))
    };
    move || {
        let path = path.clone();
        modified.get().then(|| {
            view! {
                <button
                    type="button"
                    class="absolute top-0 right-0 flex h-6 cursor-pointer items-center gap-1 rounded-sm px-1.5 text-xs font-semibold text-text-muted hover:bg-surface-3 hover:text-text"
                    title="Reset field"
                    aria-label="Reset field"
                    data-testid="reset-field"
                    on:click=move |_| {
                        let base = e
                            .draft
                            .with_untracked(|d| burrow_admin_proto::value::get(d.base(), &path).cloned());
                        e.local_error(&path, None);
                        match base {
                            Some(b) => e.set(&path, Some(b)),
                            None => e.remove_pruned(&path),
                        }
                    }
                >
                    <burrow_ui_kit::Icon name="revert" small=true/>
                    "Reset"
                </button>
            }
        })
    }
}
