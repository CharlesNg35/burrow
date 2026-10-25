use burrow_admin_proto::Path;
use burrow_admin_proto::schema::Node;
use burrow_admin_proto::serde_json::Value;
use burrow_ui_kit::FieldRow;
use leptos::prelude::*;

use crate::store::store;

use super::{SERVICE_VISITORS, editing_now, field_id, join};

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
                    label=Signal::derive(move || if is_set.get() { "In use".to_string() } else { "Not set".to_string() })
                    on_change=move |on: bool| {
                        e.set(&p, on.then(|| Value::Object(Default::default())));
                    }
                />
            }
            .into_any()
        })
    }
}

pub(super) fn visibility_row(bind: Path, public: RwSignal<bool>) -> AnyView {
    let e = store().edit;
    let editing = editing_now();
    let can = store().can_write_now();
    let kept: StoredValue<Option<Value>> = StoredValue::new(e.peek(&bind));
    let id = format!("{}-visibility", field_id(&bind));
    let visitors = {
        let mut p = bind.clone();
        p.pop();
        p.push(SERVICE_VISITORS.to_string());
        p
    };
    let website = {
        let mut p = bind.clone();
        p.pop();
        p.push(burrow_config::ServiceEntry::WEB.key().to_string());
        Memo::new(move |_| e.value(&p).is_some_and(|v| !v.is_null()))
    };
    let private_label = move || {
        if website.get() {
            "Website only"
        } else {
            "Private"
        }
    };
    let none_picked = {
        let visitors = visitors.clone();
        Memo::new(move |_| {
            e.value(&visitors)
                .is_none_or(|v| v.as_array().is_some_and(Vec::is_empty) || v.is_null())
        })
    };
    let control = {
        let bind = bind.clone();
        let id = id.clone();
        let visitors = visitors.clone();
        move || {
            let bind = bind.clone();
            let visitors = visitors.clone();
            let on = Signal::derive(move || {
                if public.get() {
                    "public".to_string()
                } else {
                    "private".to_string()
                }
            });
            if !(can && editing.try_get().unwrap_or(false)) {
                return view! { <div class="flex min-h-[38px] items-center text-[13px]">{move || if public.get() { "Public port" } else { private_label() }}</div> }.into_any();
            }
            let to_visitors = visitors.clone();
            view! {
                <div id=id.clone()>
                    <burrow_ui_kit::Segmented
                        name=id.clone()
                        label="Visibility"
                        options=vec![("public".to_string(), "Public port".to_string(), false), ("private".to_string(), private_label().to_string(), false)]
                        value=on
                        on_change=Callback::new(move |v: String| {
                            if v == "public" {
                                public.set(true);
                                if let Some(old) = kept.get_value() {
                                    e.set(&bind, Some(old));
                                }
                            } else {
                                kept.set_value(e.peek(&bind));
                                public.set(false);
                                e.set(&bind, None);
                                if none_picked.get_untracked() && !website.get_untracked() {
                                    crate::screens::entity::go_to_field(&visitors);
                                }
                            }
                        })
                    />
                </div>
                {move || (!public.get() && none_picked.get() && !website.get()).then(|| {
                    let to = to_visitors.clone();
                    view! {
                        <p class="mt-2 flex flex-wrap items-center gap-x-2 gap-y-1 text-[12.5px] text-co-info" data-testid="pick-visitors">
                            <burrow_ui_kit::Icon name="info" small=true/>
                            "Private: only the visitors you pick reach it, with burrow visit."
                            <button type="button" class="cursor-pointer font-semibold text-accent-ink underline underline-offset-2" on:click=move |_| crate::screens::entity::go_to_field(&to)>"Pick visitors"</button>
                        </p>
                    }
                })}
            }
            .into_any()
        }
    };
    view! {
        <FieldRow id=id.clone() label="Visibility" path=format!("{}-visibility", join(&bind)) schema_path="services/*/visibility">
            {control}
        </FieldRow>
    }
    .into_any()
}

pub(super) fn locked_box(node: Node, path: Path, id: String) -> AnyView {
    let e = store().edit;
    view! {
        <div class="group relative flex h-[38px] items-center rounded-input border border-border bg-surface-2 px-2.5 font-mono text-[13px] dark:bg-surface-3" id=id data-locked="">
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
