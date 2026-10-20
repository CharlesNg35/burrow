use burrow_admin_proto::serde_json::json;
use burrow_admin_proto::{PROTOCOL_MAX, SCHEMA_VERSION};
use burrow_config::admin_schema::AdminEnum;
use burrow_ui_kit::{Button, Card, Fact, Facts, PageHeader, copy_text};
use leptos::prelude::*;

use crate::format;
use crate::store::store;

const UI_VERSION: &str = env!("CARGO_PKG_VERSION");

#[component]
pub fn About() -> impl IntoView {
    let s = store();
    let diagnostics = move || {
        let w = s.welcome.get_untracked();
        let st = s.state.get_untracked();
        let schema = s.schema.get_untracked();
        json!({
            "ui": UI_VERSION,
            "server": w.as_ref().map(|w| &w.server.version),
            "target": w.as_ref().map(|w| &w.server.target),
            "protocol": w.as_ref().map(|w| w.protocol),
            "features": w.as_ref().map(|w| &w.features),
            "schema_hash": schema.as_ref().map(|s| &s.hash),
            "schema_version": schema.as_ref().map(|s| s.schema_version),
            "file": st.as_ref().map(|s| json!({"format": s.file.format, "writable": s.file.writable, "running_ok": s.running.ok})),
        })
        .to_string()
    };
    let copied = RwSignal::new(false);
    let rows = move || {
        let w = s.welcome.get()?;
        let st = s.state.get()?;
        let schema = s.schema.get()?;
        let level = w.you.level.wire();
        let writable = if st.file.writable {
            "yes".to_string()
        } else {
            st.file
                .why_not
                .as_ref()
                .map(|w| format!("no: {}", w.message))
                .unwrap_or_else(|| "no".into())
        };
        let facts: Vec<(&str, String)> = vec![
            ("Server version", w.server.version.clone()),
            ("Server target", w.server.target.clone()),
            ("Uptime", format::span(w.server.uptime_secs)),
            ("burrow admin UI", UI_VERSION.to_string()),
            (
                "Admin protocol",
                format!("{} (this UI speaks up to {PROTOCOL_MAX})", w.protocol),
            ),
            (
                "Schema",
                format!("version {} · {}", schema.schema_version, schema.hash),
            ),
            ("This UI's schema version", SCHEMA_VERSION.to_string()),
            (
                "Signed in as",
                format!("visitor {} ({level}) from {}", w.you.visitor, w.you.from),
            ),
            ("Config file", st.file.path.clone()),
            ("Format", st.file.format.clone()),
            ("Writable", writable),
            (
                "History",
                st.file
                    .history_dir
                    .clone()
                    .or(st.file.why_not_history.clone())
                    .unwrap_or_else(|| "kept in memory only".into()),
            ),
            ("Revision", st.rev.clone()),
        ];
        let features = w.features.join(", ");
        Some(view! {
            <Facts testid="about">
                {facts
                    .into_iter()
                    .map(|(k, v)| view! { <Fact label=k mono=true>{v}</Fact> })
                    .collect_view()}
                <Fact label="Features" mono=true><span data-testid="features">{features}</span></Fact>
            </Facts>
        })
    };
    view! {
        <div class="max-w-[1120px] px-8 pt-7 pb-12 max-sm:px-4" data-testid="about-page">
        <PageHeader title="About" subtitle="Versions and where the config lives"/>
        <Card>
            {rows}
            <Button
                label=Signal::derive(move || if copied.get() { "Copied".to_string() } else { "Copy diagnostics".to_string() })
                testid="copy-diagnostics"
                on_click=move |_| {
                    copy_text(&diagnostics());
                    copied.set(true);
                }
            />
        </Card>
        </div>
    }
}
