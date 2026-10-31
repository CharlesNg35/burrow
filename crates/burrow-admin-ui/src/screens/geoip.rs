use leptos::prelude::*;

use crate::store::store;

pub fn geoip_card() -> impl IntoView {
    let s = store();
    let status = Memo::new(move |_| {
        s.status.with(|st| {
            st.as_ref().and_then(|st| {
                let g = st.geoip.clone()?;
                let refused: Vec<String> = st
                    .services
                    .iter()
                    .filter_map(|v| {
                        v.countries
                            .as_ref()
                            .map(|c| format!("{}: {c}, {} refused", v.name, v.refused_country))
                    })
                    .collect();
                Some((g, refused))
            })
        })
    });
    move || {
        let (g, refused) = status.get()?;
        let now = (js_sys::Date::now() / 1000.0) as u64;
        let warn = !g.ready() || g.stale || g.failed;
        let tone = if warn { "text-warning" } else { "text-text" };
        let closed = (!g.ready()).then(|| view! {
            <p class="text-[13px] text-warning" role="status" data-testid="geoip-not-ready">"Services with allow_countries refuse public visitors until the database is ready."</p>
        });
        let file = (g.credit && !g.file.is_empty())
            .then(|| super::local_ca::fact("File", g.file.clone(), "geoip-file"));
        let credit = g.credit.then(|| view! {
            <p class="text-xs text-text-muted" data-testid="geoip-credit">
                <a href="https://db-ip.com" target="_blank" rel="noopener noreferrer" class="font-semibold text-accent-ink">"IP Geolocation by DB-IP"</a>
                ", licensed CC BY 4.0."
            </p>
        });
        Some(view! {
            <section class="mb-4 scroll-mt-20 rounded-card border border-border bg-surface" data-card="country-db" data-testid="country-db">
                {burrow_ui_kit::card_head("Country database".into(), None, ().into_any())}
                <div class="flex flex-col gap-1.5 px-4 py-3">
                    <p class=format!("text-[13px] {tone}") data-testid="geoip-line">{g.line(now)}</p>
                    {closed}
                    {file}
                    {refused.into_iter().map(|r| view! { <p class="text-[13px] text-text-muted" data-testid="geoip-service">{r}</p> }).collect_view()}
                    {credit}
                </div>
            </section>
        })
    }
}
