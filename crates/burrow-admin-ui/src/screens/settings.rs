use burrow_ui_kit::{Card, PageHeader};
use leptos::prelude::*;

use crate::form::object_view_skipping;
use crate::screens::ctx;
use crate::store::store;

#[component]
pub fn Settings() -> impl IntoView {
    let s = store();
    let body = move || {
        let c = ctx(s)?;
        let sections: Vec<String> = c
            .schema
            .entities
            .iter()
            .filter_map(|e| e.path.first().cloned())
            .collect();
        let skip: Vec<&str> = sections.iter().map(String::as_str).collect();
        let doc = c.doc.clone();
        Some(object_view_skipping(
            &c.schema.root,
            Some(&doc),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            &c,
            &skip,
        ))
    };
    view! {
        <PageHeader title="Settings" subtitle="Server-wide settings from server.yaml"/>
        <Card testid="settings">{body}</Card>
    }
}
