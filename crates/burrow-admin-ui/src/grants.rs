use burrow_admin_logic::grant::{IN_THE_FILE, would_grant};
use burrow_admin_proto::serde_json::{Map, Value};
use burrow_admin_proto::value::get;
use leptos::prelude::*;

use crate::store::store;

pub fn grants_at(entity: &[String], path: &[String], value: Option<Value>) -> bool {
    if entity.is_empty() || !path.starts_with(entity) {
        return false;
    }
    store().edit.draft.with(|d| {
        let mut candidate = get(d.draft(), entity)
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        burrow_admin_proto::value::put(&mut candidate, &path[entity.len()..], value);
        would_grant(get(d.base(), entity), &candidate)
    })
}

pub fn in_the_file() -> impl IntoView {
    view! {
        <p class="mt-1.5 flex items-center gap-1.5 text-xs text-text-muted" data-testid="grant-lock">
            <burrow_ui_kit::Icon name="lock" small=true/>
            {IN_THE_FILE}
        </p>
    }
}
