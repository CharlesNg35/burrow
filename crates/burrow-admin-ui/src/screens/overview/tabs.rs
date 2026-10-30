use burrow_admin_proto::schema::Entity;
use burrow_admin_proto::serde_json::Value;
use burrow_config::{
    ClientEntry, Entry, ServerConfig, ServiceEntry, VisitorEntry, entity_of_section,
};
use burrow_ui_kit::PillTabs;
use leptos::prelude::*;

use crate::route::{self, Link, Query, Route};
use crate::store::store;

use super::certs::certificates_panel;
use super::tables::{clients_table, layouts, services_table, visitors_table};
use super::web::websites_panel;

const WEBSITES: &str = "websites";
const CERTIFICATES: &str = "certs";

fn has_web(values: &Value) -> bool {
    ServerConfig::WEB.get(values).is_some_and(|w| !w.is_null())
}

fn asked_tab() -> &'static str {
    let asked = route::query(route::TAB_QUERY);
    match asked.as_deref() {
        Some(WEBSITES) => WEBSITES,
        Some(CERTIFICATES) => CERTIFICATES,
        Some(t) => match entity_of_section(t) {
            Some(Entity::Client) => ClientEntry::section(),
            Some(Entity::Visitor) => VisitorEntry::section(),
            _ => ServiceEntry::section(),
        },
        None => ServiceEntry::section(),
    }
}

fn open_link(tab: &'static str) -> (Route, Query, String) {
    match tab {
        WEBSITES => (
            Route::List(ServiceEntry::section().into()),
            Some((route::FILTER_QUERY, route::WEBSITES_FILTER)),
            format!("Open {WEBSITES} →"),
        ),
        CERTIFICATES => (
            Route::Settings,
            Some((route::CARD_QUERY, ServerConfig::WEB.key())),
            "Open certificates →".to_string(),
        ),
        t => (Route::List(t.to_string()), None, format!("Open {t} →")),
    }
}

pub(super) fn tab_box(values: Memo<Value>, counts: Signal<Vec<String>>) -> impl IntoView {
    let s = store();
    let web = Memo::new(move |_| has_web(&values.get()));
    let tab = RwSignal::new(asked_tab());
    let shown = Signal::derive(move || {
        let t = tab.get();
        if (t == WEBSITES || t == CERTIFICATES) && !web.get() {
            ServiceEntry::section()
        } else {
            t
        }
    });
    let all_counts = Memo::new(move |_| {
        let mut c = counts.get();
        if web.get() {
            let now = (s.now.get() / 1000.0) as u64;
            s.status.with(|st| {
                let st = st.as_deref();
                let sites = st.and_then(|st| st.web.as_ref()).map(|w| w.hosts.len());
                c.push(sites.map(|n| n.to_string()).unwrap_or_default());
                c.push(
                    st.map(|st| burrow_admin_logic::certs::count(st, now))
                        .unwrap_or_default(),
                );
            });
        }
        c
    });
    let pills = move || {
        let mut tabs: Vec<(&'static str, String)> = [
            ServiceEntry::section(),
            ClientEntry::section(),
            VisitorEntry::section(),
        ]
        .map(|t| (t, crate::screens::plural(t).to_string()))
        .into();
        if web.get() {
            tabs.push((WEBSITES, "Websites".to_string()));
            tabs.push((CERTIFICATES, "Certificates".to_string()));
        }
        view! {
            <PillTabs
                tabs=tabs
                active=shown
                counts=all_counts.into()
                on_change=Callback::new(move |t: &'static str| {
                    tab.set(t);
                    route::set_query(route::TAB_QUERY, t);
                })
            />
        }
    };
    let total = move |t: &'static str| match t {
        WEBSITES => s.status.with(|st| {
            st.as_ref()
                .and_then(|st| st.web.as_ref())
                .map_or(0, |w| w.hosts.len())
        }),
        CERTIFICATES => {
            let now = (s.now.get() / 1000.0) as u64;
            s.status.with(|st| {
                st.as_deref()
                    .map_or(0, |st| burrow_admin_logic::certs::rows(st, now).len())
            })
        }
        t => super::names(&values.get(), t).len(),
    };
    let open = move || {
        let t = shown.get();
        let cut = total(t) > burrow_admin_logic::top::ROWS;
        let rates = entity_of_section(t).is_some().then_some(if cut {
            "current rates"
        } else {
            "current rates · "
        });
        let (to, query, label) = open_link(t);
        view! {
            {rates}
            {(!cut).then(|| view! { <Link to=to query=query class="font-semibold text-accent-ink no-underline hover:underline" testid="overview-open">{label}</Link> })}
        }
    };
    let layouts = layouts(s, values);
    let panel = move || match (shown.get(), entity_of_section(shown.get())) {
        (WEBSITES, _) => websites_panel(
            s.status
                .with(|st| st.as_ref().and_then(|st| st.web.clone())),
        )
        .into_any(),
        (CERTIFICATES, _) => {
            let now = (s.now.get() / 1000.0) as u64;
            s.status
                .with(|st| certificates_panel(st.as_deref(), now))
                .into_any()
        }
        (_, Some(Entity::Client)) => clients_table(s, values, layouts.clients).into_any(),
        (_, Some(Entity::Visitor)) => visitors_table(s, values, layouts.visitors).into_any(),
        _ => services_table(values, layouts.services).into_any(),
    };
    view! {
        <section class="col-span-2 rounded-card border border-border bg-surface md:col-span-6 xl:col-span-12" data-testid="overview-tabs">
            <div class="flex flex-wrap items-center gap-3 border-b border-border px-3 py-2.5">
                {pills}
                <span class="ml-auto text-xs text-text-muted">{open}</span>
            </div>
            <div class="@container" role="tabpanel">{panel}</div>
        </section>
    }
}
