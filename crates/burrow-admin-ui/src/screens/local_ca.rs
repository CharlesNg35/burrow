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
    let parts: Vec<String> = value.split(':').map(str::to_string).collect();
    let n = parts.len();
    let shown = parts
        .into_iter()
        .enumerate()
        .map(|(i, p)| view! { {p}{(i + 1 < n).then(|| view! { ":"<wbr/> })} })
        .collect_view();
    view! {
        <p class="flex flex-col gap-x-2 text-[13px] sm:flex-row">
            <span class="w-28 shrink-0 font-medium">{label}</span>
            <span class="font-mono" data-testid=testid>{shown}</span>
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
            view! { "No site uses it yet." }.into_any()
        } else {
            let n = sites.len();
            let links = sites
                .into_iter()
                .enumerate()
                .map(|(i, s)| view! {
                    {(i > 0).then_some(", ")}
                    <crate::route::Link to=crate::route::Route::Detail(ServiceEntry::section().into(), s.clone()) class="font-semibold text-accent-ink no-underline hover:underline">{s}</crate::route::Link>
                })
                .collect_view();
            view! { {format!("Used by {n} site{}: ", if n == 1 { "" } else { "s" })}{links} }
                .into_any()
        };
        let pending = ca.is_none().then(|| view! {
            <p class="text-[13px] text-text-muted" data-testid="local-ca-pending">"No local CA exists yet. burrow makes one when the first site with HTTPS: Local CA is applied."</p>
        });
        let download = ca.is_some().then(|| view! {
            <burrow_ui_kit::Button label="Download CA certificate" icon="download" small=true testid="local-ca-download" on_click=move |_| crate::uploads::download_local_ca()/>
        });
        view! {
            <section class="mb-4 scroll-mt-20 rounded-card border border-border bg-surface" data-card="local_ca" data-testid="local-ca">
                {burrow_ui_kit::card_head("Local certificate authority".into(), None, ().into_any())}
                <div class="flex flex-col gap-1.5 px-4 py-3">
                    {pending}
                    {facts}
                    <p class="text-[13px]" data-testid="local-ca-sites">{used}</p>
                    {ca.is_some().then(|| view! {
                        <p class="text-xs text-text-muted">
                            "Install this certificate once on each device that opens these sites; "
                            <a href=burrow_admin_logic::wording::docs("web/certificates.md") target="_blank" rel="noopener noreferrer" class="font-semibold text-accent-ink" data-testid="local-ca-docs">"how to install it on each system and browser"</a>
                            ". Compare the SHA-256 above with what the device shows."
                        </p>
                    })}
                    <div>{download}</div>
                </div>
            </section>
        }
    }
}
