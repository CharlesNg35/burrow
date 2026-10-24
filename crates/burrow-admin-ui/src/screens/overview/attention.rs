use burrow_config::{ClientEntry, Entry};
use burrow_ui_kit::{Icon, Tone, tag_class};
use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::format;
use crate::route::Link;
use crate::store::store;

use super::{Item, attention, away, names, old_server_item};

fn row_view(i: Item, icon: bool) -> impl IntoView {
    let tone = i.tone;
    let glyph = match tone {
        Tone::Danger => "alert-circle",
        Tone::Warning => "alert",
        Tone::Info => "info",
        _ => "monitor",
    };
    let inner = view! {
        <span class=format!("mt-0.5 shrink-0 {}", tone.text())>
            {if icon { view! { <Icon name=glyph/> }.into_any() } else { view! { <span class="mt-1 block size-2 rounded-full bg-text-faint" aria-hidden="true"></span> }.into_any() }}
        </span>
        <span class="min-w-0">
            <span class="block text-sm font-semibold">{i.title.clone()}</span>
            <span class="block text-[12.5px] text-text-muted">{i.detail.clone()}</span>
        </span>
    };
    let class = "flex gap-2.5 px-4 py-[11px] text-text no-underline hover:bg-surface-2 dark:hover:bg-surface-3/50";
    let li = "border-b border-rowline last:border-b-0";
    view! {
        <li class=li data-item=i.key.clone() data-tone=tone.name()>
            {match (i.to, i.field) {
                (Some(to), Some(path)) => {
                    let href = crate::route::href(&to);
                    view! {
                        <a href=href class=class on:click=move |e| {
                            e.prevent_default();
                            crate::route::go(to.clone());
                            let path = path.clone();
                            set_timeout(move || crate::screens::entity::go_to_field(&path), std::time::Duration::from_millis(150));
                        }>{inner}</a>
                    }
                    .into_any()
                }
                (Some(to), None) => view! { <Link to=to class=class>{inner}</Link> }.into_any(),
                (None, _) => view! { <div class=class>{inner}</div> }.into_any(),
            }}
        </li>
    }
}

fn numbered(items: Vec<Item>) -> Vec<(usize, Item)> {
    items.into_iter().enumerate().collect()
}

fn hidden_below(list: &web_sys::HtmlElement) -> usize {
    let bottom = list.scroll_top() + list.client_height();
    let Ok(items) = list.query_selector_all("li") else {
        return 0;
    };
    (0..items.length())
        .filter_map(|i| {
            items
                .get(i)
                .and_then(|n| n.dyn_into::<web_sys::HtmlElement>().ok())
        })
        .filter(|el| el.offset_height() > 0 && el.offset_top() + el.offset_height() > bottom + 2)
        .count()
}

#[component]
fn Section(
    title: &'static str,
    #[prop(into)] items: Signal<Vec<Item>>,
    tone: Signal<Tone>,
    icon: bool,
    testid: &'static str,
    #[prop(into)] grow: Signal<String>,
    empty: (&'static str, Signal<String>, Signal<String>),
) -> impl IntoView {
    let list: NodeRef<html::Ul> = NodeRef::new();
    let hidden = RwSignal::new(0usize);
    let measure = move || {
        if let Some(el) = list.try_get_untracked().flatten() {
            let el: web_sys::HtmlElement = el.into();
            let n = if el.client_height() > 0 && el.scroll_height() > el.client_height() {
                hidden_below(&el)
            } else {
                0
            };
            if hidden.try_get_untracked().is_some_and(|h| h != n) {
                let _ = hidden.try_set(n);
            }
        }
    };
    Effect::new(move |_| {
        items.track();
        request_animation_frame(measure);
    });
    let resize = window_event_listener(leptos::ev::resize, move |_| measure());
    on_cleanup(move || resize.remove());
    let total = move || items.with(Vec::len);
    view! {
        <div class=move || format!("flex min-h-0 flex-col {}", grow.get()) data-testid=testid>
            <div class="flex shrink-0 items-center gap-2 border-b border-border px-4 py-3">
                <h2 class="text-[13.5px] font-bold">{title}</h2>
                <span class=move || tag_class(tone.get()) data-testid="count">{total}</span>
                <span class="ml-auto"></span>
            </div>
            <div class="relative min-h-0 flex-1">
                {move || if total() == 0 {
                    view! {
                        <div class="flex h-full items-center justify-center py-6">
                            <div class="flex flex-col items-center gap-1.5 px-4 text-center">
                                <span class="text-success"><Icon name=empty.0 large=true/></span>
                                <span class="text-sm font-semibold">{move || empty.1.get()}</span>
                                <span class="text-[12.5px] text-text-muted">{move || empty.2.get()}</span>
                            </div>
                        </div>
                    }.into_any()
                } else {
                    ().into_any()
                }}
                <ul
                    node_ref=list
                    class="h-full overflow-y-auto overscroll-contain [scrollbar-width:thin] max-[1199px]:max-h-[340px]"
                    on:scroll=move |_| measure()
                >
                    <For
                        each=move || numbered(items.get())
                        key=|(i, x)| format!("{i}|{}|{}|{}", x.key, x.title, x.detail)
                        children=move |(_, x)| row_view(x, icon)
                    />
                </ul>
                <div data-testid="fade" class=move || format!("pointer-events-none absolute inset-x-0 bottom-0 h-6 bg-gradient-to-t from-surface to-transparent{}", if hidden.get() > 0 { "" } else { " hidden" })></div>
            </div>
        </div>
    }
}

#[component]
pub(super) fn Attention() -> impl IntoView {
    let s = store();
    let items = Memo::new(move |_| {
        let v = s.running();
        let st = s.state.get();
        let status = s.status.get();
        let now = (s.now.get() / 1000.0) as u64;
        let mut items = attention(&v, st.as_deref(), status.as_deref(), now);
        items.extend(s.welcome.with(|w| {
            w.as_ref()
                .and_then(|w| old_server_item(&w.server.version, env!("CARGO_PKG_VERSION")))
        }));
        items
    });
    let away_items = Memo::new(move |_| {
        let v = s.running();
        away(&v, s.status.get().as_deref())
    });
    let seen: StoredValue<Vec<String>> = StoredValue::new(Vec::new());
    let announce = RwSignal::new(String::new());
    Effect::new(move |_| {
        let now: Vec<Item> = items
            .get()
            .into_iter()
            .filter(|i| i.tone == Tone::Danger)
            .collect();
        let before = seen.get_value();
        let fresh: Vec<String> = now
            .iter()
            .filter(|i| !before.contains(&i.key))
            .map(|i| i.title.clone())
            .collect();
        if !fresh.is_empty() && !before.is_empty() {
            announce.set(fresh.join(". "));
        }
        seen.set_value(
            now.into_iter()
                .map(|i| i.key)
                .chain(std::iter::once(String::new()))
                .collect(),
        );
    });
    let longest = move || {
        s.status.with(|st| {
            st.as_ref().and_then(|st| {
                st.clients
                    .iter()
                    .filter(|c| c.connected && !c.local)
                    .max_by_key(|c| c.connected_secs.unwrap_or(0))
                    .map(|c| {
                        format!(
                            "Longest connected: {}, {}.",
                            c.name,
                            format::span(c.connected_secs.unwrap_or(0))
                        )
                    })
            })
        })
    };
    let n_clients = move || names(&s.running(), ClientEntry::section()).len();
    let away_title: &'static str = "Clients away";
    let away_grow = Signal::derive(move || {
        if away_items.with(Vec::len) >= 2 {
            "shrink-[2] grow basis-auto min-[1200px]:min-h-[156px]".to_string()
        } else {
            "shrink-0 grow basis-auto".to_string()
        }
    });
    let away_tone = Signal::derive(move || {
        if away_items.with(Vec::is_empty) {
            Tone::Success
        } else {
            Tone::Warning
        }
    });
    let away_title_empty = Signal::derive(move || {
        let n = n_clients();
        if n == 0 {
            "No clients yet".to_string()
        } else {
            format!("All {n} clients online")
        }
    });
    let away_msg = Signal::derive(move || longest().unwrap_or_default());
    view! {
        <section class="relative rounded-card border border-border bg-surface min-[1200px]:col-span-4" data-testid="attention-card">
            <div class="flex flex-col overflow-hidden rounded-card min-[1200px]:absolute min-[1200px]:inset-0 [&>[data-testid=away]]:border-t [&>[data-testid=away]]:border-border">
                <Section
                    title="Needs attention"
                    items=items
                    tone=Signal::derive(move || if items.with(Vec::is_empty) { Tone::Success } else { Tone::Warning })
                    icon=true
                    testid="attention"
                    grow="grow shrink basis-auto".to_string()
                    empty=("check", Signal::stored("Nothing needs attention".to_string()), Signal::stored("Every service has a client online and every check passes.".to_string()))
                />
                <Section
                    title=away_title
                    items=away_items
                    tone=away_tone
                    icon=false
                    testid="away"
                    grow=away_grow
                    empty=("monitor", away_title_empty, away_msg)
                />
            </div>
            <p class="sr-only" aria-live="polite">{move || announce.get()}</p>
        </section>
    }
}
