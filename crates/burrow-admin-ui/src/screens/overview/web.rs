use burrow_admin_proto::web::WebSummary;
use burrow_config::{Entry, ServiceEntry};
use leptos::prelude::*;

use crate::route::{Link, Route};
use crate::store::store;

fn rows(web: WebSummary) -> impl IntoView {
    let tunnel = web.tunnel.clone().map(|t| {
        view! {
            <li class="flex flex-wrap items-baseline gap-x-3 gap-y-0.5 px-4 py-2.5" data-web-row="tunnel">
                <span class="font-mono text-[13px]">{t.url}</span>
                <span class="text-xs text-text-muted">"the tunnel, for burrow clients"</span>
            </li>
        }
    });
    let empty = (web.hosts.is_empty() && web.tunnel.is_none()).then(|| {
        view! { <li class="px-4 py-2.5 text-[13px] text-text-muted">"No names yet: every connection to this port is closed."</li> }
    });
    let hosts = web
        .hosts
        .into_iter()
        .map(|h| {
            let to = Route::Detail(ServiceEntry::section().into(), h.service.clone());
            let (row, kind) = (h.name.clone(), format!("{}, {}", h.kind, h.certificate));
            view! {
                <li class="flex flex-wrap items-baseline gap-x-3 gap-y-0.5 px-4 py-2.5" data-web-row=row>
                    <span class="font-mono text-[13px]">{h.name}</span>
                    <span class="text-xs text-text-muted">{kind}</span>
                    <span class="ml-auto text-[13px]">"→ "<Link to=to class="font-semibold text-accent-ink no-underline hover:underline">{h.service}</Link></span>
                </li>
            }
        })
        .collect_view();
    view! { <ul class="divide-y divide-border">{tunnel}{hosts}{empty}</ul> }
}

#[component]
pub fn Websites() -> impl IntoView {
    let s = store();
    let web = Memo::new(move |_| {
        s.status
            .with(|st| st.as_ref().and_then(|st| st.web.clone()))
    });
    move || {
        web.get().map(|w| {
            view! {
                <section class="col-span-2 rounded-card border border-border bg-surface md:col-span-6 xl:col-span-12" data-testid="websites">
                    <header class="flex flex-wrap items-baseline gap-x-3 border-b border-border px-4 py-3">
                        <h2 class="text-sm font-semibold">"Websites"</h2>
                        <span class="text-xs text-text-muted" data-testid="websites-port">{w.head().trim_start_matches("web: ").to_string()}</span>
                    </header>
                    {rows(w.clone())}
                </section>
            }
        })
    }
}
