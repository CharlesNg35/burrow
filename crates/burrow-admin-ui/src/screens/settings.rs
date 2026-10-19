use burrow_ui_kit::{Card, PageHeader};
use leptos::prelude::*;

use crate::form::object_view_skipping;
use crate::store::store;

#[component]
pub fn Settings() -> impl IntoView {
    let s = store();
    let body = move || {
        let schema = s.schema.get()?;
        let sections: Vec<String> = schema
            .entities
            .iter()
            .filter_map(|e| e.path.first().cloned())
            .collect();
        let skip: Vec<&str> = sections.iter().map(String::as_str).collect();
        Some(object_view_skipping(
            &schema.root,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            &skip,
        ))
    };
    view! {
        <PageHeader title="Settings" subtitle="Server-wide settings from server.yaml"/>
        <Card testid="settings">{body}</Card>
    }
}
