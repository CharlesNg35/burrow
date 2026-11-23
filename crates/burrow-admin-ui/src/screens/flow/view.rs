use burrow_ui_kit::{Icon, Modal, Segmented, Tooltip, button_class};
use leptos::html;
use leptos::prelude::*;

use crate::screens::search_box;
use crate::store::{Store, store};

use super::draw::{Drawn, Hover, flow_links, flow_svg};
use super::{Chart, SEARCH, layout};

#[component]
pub fn FlowModal() -> impl IntoView {
    let s = store();
    let only_problems = RwSignal::new(false);
    let query = RwSignal::new(String::new());
    let chart = Memo::new(move |_| {
        if !s.flow_open.get() {
            return Chart::default();
        }
        let doc = s.running();
        let status = s.status.get();
        let rates = s.rates.get();
        query.with(|q| layout(&doc, status.as_deref(), &rates, s.me().as_deref(), q))
    });
    let dr = Drawn::new(chart);
    let panel: NodeRef<html::Div> = NodeRef::new();
    let wrap: NodeRef<html::Div> = NodeRef::new();
    let hover: Hover = RwSignal::new(None);
    restore_focus(s, chart, only_problems, query, hover);
    forget_hover(dr, hover);
    let tip = Memo::new(move |_| {
        hover
            .get()
            .and_then(|(k, x, y)| dr.tip(&k).map(|t| (t, x, y)))
    });
    let close = move || s.flow_open.set(false);
    let filter = Signal::derive(move || {
        if only_problems.get() {
            "problems".to_string()
        } else {
            "all".to_string()
        }
    });
    let searching = move || query.with(|q| !q.trim().is_empty());
    view! {
        <Modal
            open=s.flow_open
            on_close=Callback::new(move |_: ()| close())
            label="Flow".to_string()
            class="fixed top-14 right-24 left-24 z-50 flex max-h-[calc(100vh-80px)] flex-col rounded-[16px] border border-border-strong bg-surface shadow-overlay max-lg:right-4 max-lg:left-4 max-sm:inset-0 max-sm:max-h-none max-sm:rounded-none dark:bg-surface-2"
            testid="flow"
            panel=panel
        >
                <div class="flex flex-wrap items-center gap-3 border-b border-border px-5 py-3.5">
                    <span class="text-accent-ink"><Icon name="network"/></span>
                    <h2 class="text-[17px] font-bold">"Flow"</h2>
                    <span class="text-sm text-text-muted">"live · every connection from source to target"</span>
                    <span class="ml-auto"></span>
                    {move || {
                        let n = chart.with(|c| c.problems);
                        view! {
                            <Segmented
                                name="flow-filter"
                                label="Show"
                                options=vec![("all".to_string(), "All flows".to_string(), false), ("problems".to_string(), format!("Problems {n}"), false)]
                                value=filter
                                on_change=Callback::new(move |v: String| only_problems.set(v == "problems"))
                            />
                        }
                    }}
                    <div class="flex w-64 items-center gap-2 max-sm:order-last max-sm:w-full" data-testid="flow-search">
                        <div class="min-w-0 flex-1">{search_box(SEARCH, query, "Search services or clients".into(), Signal::stored(false))}</div>
                        <span class="shrink-0 text-xs text-text-muted tabular-nums" data-testid="flow-count" class:hidden=move || !searching()>
                            {move || chart.with(|c| format!("{} of {}", c.services, c.total))}
                        </span>
                    </div>
                    <button type="button" class=button_class(burrow_ui_kit::ButtonLook::Ghost, burrow_ui_kit::ButtonSize::Small) aria-label="Close" data-testid="flow-close" on:click=move |_| close()>
                        <Icon name="x"/>
                    </button>
                </div>
                <div node_ref=wrap class="relative min-h-0 flex-1 overflow-auto overscroll-contain px-[22px] pt-4 pb-1.5">
                    {move || (searching() && chart.with(|c| c.services == 0)).then(|| view! {
                        <p class="pb-2 text-sm text-text-muted" data-testid="flow-none">{format!("No service or client matches \u{201c}{}\u{201d}.", query.get().trim())}</p>
                    })}
                    {flow_svg(dr, only_problems.into(), hover, wrap)}
                    {move || tip.get().map(|(t, x, y)| view! {
                        <Tooltip x=x y=y text=t/>
                    })}
                    <ul class="sr-only" aria-label="Open from the flow chart">{flow_links(s, dr)}</ul>
                </div>
                {legend()}
        </Modal>
    }
}

fn restore_focus(
    s: Store,
    chart: Memo<Chart>,
    only_problems: RwSignal<bool>,
    query: RwSignal<String>,
    hover: Hover,
) {
    Effect::new(move |was: Option<bool>| {
        let open = s.flow_open.get();
        if open && was != Some(true) {
            query.set(String::new());
            let c = chart.get_untracked();
            only_problems.set(c.services > 12 && c.problems > 0);
        } else if !open && was == Some(true) {
            hover.set(None);
        }
        open
    });
}

fn forget_hover(dr: Drawn, hover: Hover) {
    Effect::new(move |seen: Option<()>| {
        dr.node_keys.track();
        if seen.is_some() {
            hover.set(None);
        }
    });
}

fn legend() -> impl IntoView {
    view! {
    <div class="flex flex-wrap items-center gap-x-4 gap-y-1 border-t border-border px-4 py-2.5 text-xs text-text-muted">
        <span class="flex items-center gap-1.5"><span class="h-0.5 w-4 rounded bg-success"></span>"healthy"</span>
        <span class="flex items-center gap-1.5"><span class="h-0.5 w-4 rounded bg-warning"></span>"degraded"</span>
        <span class="flex items-center gap-1.5"><span class="h-0.5 w-4 rounded bg-danger"></span>"down"</span>
        <span class="flex items-center gap-1.5"><span class="w-4 border-t border-dashed border-text-faint"></span>{burrow_admin_logic::wording::OFFLINE}</span>
        <span class="flex items-center gap-1.5"><span class="h-0.5 w-4 rounded bg-accent"></span>"server–client channels"</span>
        <span>"line weight = open connections; client → target lines are fixed"</span>
        <span class="ml-auto">"Click a node to open that service or client · Esc closes"</span>
    </div>
    }
}
