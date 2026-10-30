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
    let diagnostics = move || crate::app::diagnostics(s);
    let copied = RwSignal::new(false);
    let rows = move || {
        let w = s.welcome.get()?;
        let st = s.state.get()?;
        let schema = s.schema.get()?;
        let level = w.you.level.wire();
        let writable = if st.file.writable {
            "Yes".to_string()
        } else {
            st.file
                .why_not
                .as_ref()
                .map(|w| format!("No: {}", w.message))
                .unwrap_or_else(|| "No".into())
        };
        let facts: Vec<(&str, String)> = vec![
            (
                "Server version",
                if burrow_admin_logic::version::server_is_older(&w.server.version, UI_VERSION) {
                    format!(
                        "{} · older than this burrow admin, update the server",
                        w.server.version
                    )
                } else {
                    w.server.version.clone()
                },
            ),
            ("Server target", w.server.target.clone()),
            (
                "Uptime",
                format::span(
                    s.status
                        .with(|x| x.as_ref().map(|x| x.uptime_secs))
                        .unwrap_or(w.server.uptime_secs),
                ),
            ),
            ("burrow admin UI", UI_VERSION.to_string()),
            (
                "Admin protocol",
                format!("{} (this UI speaks up to {PROTOCOL_MAX})", w.protocol),
            ),
            (
                "Schema",
                format!(
                    "version {} · {}",
                    schema.schema_version,
                    schema
                        .hash
                        .trim_start_matches(burrow_admin_proto::REV_PREFIX)
                ),
            ),
            ("This UI's schema version", SCHEMA_VERSION.to_string()),
            (
                "Signed in as",
                format!("{} · admin {level} · from {}", w.you.visitor, w.you.from),
            ),
            ("Config file", st.file.path.clone()),
            ("Format", st.file.format.to_uppercase()),
            ("Writable", writable),
            (
                "History",
                st.file
                    .history_dir
                    .clone()
                    .or(st.file.why_not_history.clone())
                    .unwrap_or_else(|| "kept in memory only".into()),
            ),
            ("Revision", crate::store::short_rev(&st.rev)),
        ];
        Some(view! {
            <Facts testid="about">
                {facts
                    .into_iter()
                    .map(|(k, v)| view! { <Fact label=k mono=true>{v}</Fact> })
                    .collect_view()}
            </Facts>
        })
    };
    view! {
        <div class="max-w-[1120px] px-8 pt-7 pb-12 max-sm:px-4" data-testid="about-page">
        <PageHeader sub="Versions and where the config lives">"About"</PageHeader>
        <Card>
            {rows}
            <div class="mt-4">
            <Button
                label=Signal::derive(move || if copied.get() { "Copied".to_string() } else { "Copy diagnostics".to_string() })
                testid="copy-diagnostics"
                on_click=move |_| {
                    copy_text(&diagnostics());
                    copied.set(true);
                }
            />
            </div>
        </Card>
        </div>
    }
}
