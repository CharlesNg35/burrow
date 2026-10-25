use burrow_admin_proto::serde_json::Value;
use burrow_config::{Entry, ServiceEntry, WebSite};
use leptos::prelude::*;

use crate::store::store;

fn internal_sites(doc: &Value) -> Vec<String> {
    doc.get(ServiceEntry::SECTION)
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter(|(_, svc)| {
                    ServiceEntry::WEB
                        .get(svc)
                        .and_then(|w| WebSite::TLS.get(w))
                        .and_then(Value::as_str)
                        == Some("internal")
                })
                .map(|(name, _)| name.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn fact(label: &'static str, value: String, testid: &'static str) -> impl IntoView {
    view! {
        <p class="flex flex-wrap gap-x-2 text-[13px]">
            <span class="w-28 font-medium">{label}</span>
            <span class="break-all font-mono" data-testid=testid>{value}</span>
        </p>
    }
}

pub fn local_ca_card() -> impl IntoView {
    let s = store();
    let ca = Memo::new(move |_| {
        s.status
            .with(|st| st.as_ref().and_then(|st| st.local_ca.clone()))
    });
    let sites = Memo::new(move |_| internal_sites(&s.edit.doc()));
    move || {
        let ca = ca.get();
        let sites = sites.get();
        if ca.is_none() && sites.is_empty() {
            return None;
        }
        let facts = ca.clone().map(|c| {
            view! {
                {fact("Name", c.subject.clone(), "local-ca-subject")}
                {fact("SHA-256", c.sha256.clone(), "local-ca-fingerprint")}
                {fact("Created", crate::format::day(c.not_before), "local-ca-created")}
                {fact("Expires", crate::format::day(c.not_after), "local-ca-expires")}
                {c.warning.clone().map(|w| view! { <p class="text-[13px] text-warning" role="status" data-testid="local-ca-warning">{w}</p> })}
            }
        });
        let used = if sites.is_empty() {
            "No site uses it yet.".to_string()
        } else {
            format!("Used by {}.", sites.join(", "))
        };
        let pending = ca.is_none().then(|| view! {
            <p class="text-[13px] text-text-muted" data-testid="local-ca-pending">"burrow makes its CA when the server applies the first site with tls: internal."</p>
        });
        let download = ca.is_some().then(|| view! {
            <burrow_ui_kit::Button label="Download CA certificate" icon="download" small=true testid="local-ca-download" on_click=move |_| crate::uploads::download_local_ca()/>
        });
        Some(view! {
            <section class="mb-4 scroll-mt-20 rounded-card border border-border bg-surface" data-card="local_ca" data-testid="local-ca">
                <div class="flex min-h-12 items-center gap-3 border-b border-border px-4 py-3">
                    <h2 class="text-[15px] font-semibold">"Local certificate authority"</h2>
                </div>
                <div class="flex flex-col gap-1.5 px-4 py-3">
                    {pending}
                    {facts}
                    <p class="text-[13px]" data-testid="local-ca-sites">{used}</p>
                    <p class="text-xs text-text-muted">"Install this certificate once on each device that opens these sites; docs/web/certificates.md says how for each system and browser. Compare the SHA-256 above with what the device shows."</p>
                    <div>{download}</div>
                </div>
            </section>
        })
    }
}
