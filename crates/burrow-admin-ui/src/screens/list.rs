use burrow_admin_logic::wording::{WriteAction, needs_write_admin};
use burrow_config::Protocol;
use burrow_ui_kit::{EmptyState, Icon, Kbd, LABEL, Tone, tag_class};
use leptos::prelude::*;

use crate::route::{self, Link};

use super::Row;

#[component]
pub fn TocRow(row: Row, selected: bool) -> impl IntoView {
    let class = if selected {
        "bg-accent-weak text-accent-ink"
    } else {
        "text-text hover:bg-surface-3"
    };
    view! {
        <div class=format!("relative mx-3 mb-0.5 flex items-center gap-2 rounded-md px-3 py-2 {class}") data-row=row.key.clone() data-group=row.group.clone() data-selected=selected.then_some("true")>
            <Link to=row.to.clone() class="min-w-0 flex-1 truncate text-sm font-semibold text-inherit no-underline after:absolute after:inset-0 after:rounded-md" testid="list-row" current=Signal::stored(selected)>
                {row.title.clone()}
            </Link>
            {row.tags.iter().map(|(t, tone)| view! { <span class=tag_class(*tone)>{t.clone()}</span> }).collect_view()}
        </div>
    }
}

#[component]
pub fn ListCard(row: Row, selected: bool) -> impl IntoView {
    let class = if selected {
        "border-accent-line ring-3 ring-accent-weak"
    } else {
        "border-border hover:border-border-strong"
    };
    let bg = if row.pending.is_some() {
        "bg-accent-weak"
    } else {
        "bg-surface dark:bg-surface-2"
    };
    let tile = if selected {
        "bg-accent-weak text-accent-ink"
    } else {
        "bg-surface-3 text-text-muted"
    };
    let key = row.key.clone();
    view! {
        <div class=format!("relative mx-3 mb-2 grid grid-cols-[30px_minmax(0,1fr)_auto] gap-x-3 rounded-[11px] border px-3.5 py-3 {class} {bg}") data-row=key.clone() data-group=row.group.clone() data-selected=selected.then_some("true")>
            <span class=format!("relative flex size-[30px] items-center justify-center rounded-md {tile}")>
                <Icon name=row.icon/>
                {row.dot.map(|t| view! { <span class=format!("absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full ring-2 ring-surface {}", t.dot()) aria-hidden="true"></span> })}
            </span>
            <span class="min-w-0">
                <span class="flex min-w-0 items-center gap-1.5">
                    <Link to=row.to.clone() class="truncate font-semibold text-text no-underline after:absolute after:inset-0 after:rounded-[11px]" testid="list-row" current=Signal::stored(selected)>
                        {row.title.clone()}
                    </Link>
                    {row.title_tag.clone().map(|(t, tone)| view! { <span class=tag_class(tone)>{t}</span> })}
                </span>
                {(!row.line2.is_empty()).then(|| view! { <span class="block truncate text-sm text-text-muted">{row.line2.clone()}</span> })}
                {(!row.line3.is_empty()).then(|| view! { <span class="mt-0.5 block truncate text-sm text-text-muted">{row.line3.clone()}</span> })}
                {(!row.tags.is_empty() || row.pending.is_some() || !row.aside.is_empty()).then(|| view! {
                    <span class="mt-1.5 flex flex-wrap items-center gap-1.5">
                        {row.pending.map(|p| view! { <span class=tag_class(Tone::Accent) data-pending=p>{p}</span> })}
                        {row.tags.iter().map(|(t, tone)| view! { <span class=tag_class(*tone)>{t.clone()}</span> }).collect_view()}
                        {(!row.aside.is_empty()).then(|| view! { <span class="text-sm text-text-muted">{row.aside.clone()}</span> })}
                    </span>
                })}
                {(!row.actions.is_empty()).then(|| view! {
                    <span class="relative z-10 mt-1.5 flex gap-3 text-xs">
                        {row.actions.iter().map(|a| {
                            let icon = if *a == "Revert" { "revert" } else { "eye" };
                            view! {
                                <button type="button" class="inline-flex cursor-pointer items-center gap-1 font-semibold text-text-muted hover:text-accent-ink" data-action=a.to_lowercase() data-key=key.clone()>
                                    <Icon name=icon small=true/>
                                    {*a}
                                </button>
                            }
                        }).collect_view()}
                    </span>
                })}
            </span>
            <span class="font-mono text-xs whitespace-nowrap text-text-muted">{row.meta.clone()}</span>
        </div>
    }
}

#[component]
pub fn ListPane(
    #[prop(into)] title: String,
    #[prop(into)] count: Signal<String>,
    #[prop(optional, into)] new_label: Option<String>,
    #[prop(optional, into)] on_new: Option<Callback<()>>,
    #[prop(into)] search_label: String,
    filters: Vec<(&'static str, String)>,
    groups: Vec<&'static str>,
    #[prop(into)] rows: Signal<Vec<Row>>,
    #[prop(into)] selected: Signal<Option<String>>,
    #[prop(optional, into)] empty: Option<(String, String)>,
    #[prop(optional, into)] on_action: Option<Callback<(String, String)>>,
    #[prop(optional)] testid: &'static str,
    #[prop(optional)] no_counts: bool,
    #[prop(optional)] new_disabled: bool,
    #[prop(optional)] default_filter: Option<&'static str>,
    #[prop(optional)] compact: bool,
    #[prop(optional)] empty_icon: Option<&'static str>,
    #[prop(optional)] query: Option<RwSignal<String>>,
    #[prop(default = None)] new_menu: Option<Vec<NewChoice>>,
) -> impl IntoView {
    let q = query.unwrap_or_else(|| RwSignal::new(String::new()));
    let asked = route::query(route::FILTER_QUERY);
    let first = filters
        .iter()
        .map(|(k, _)| *k)
        .find(|k| asked.as_deref() == Some(*k));
    let filter = RwSignal::new(first.or(default_filter).unwrap_or("all"));
    let visible = Memo::new(move |_| {
        let needle = q.get().to_lowercase();
        let f = filter.get();
        rows.get()
            .into_iter()
            .filter(|r| r.matches(&needle) && (f == "all" || r.filters.contains(&f)))
            .collect::<Vec<_>>()
    });
    let move_sel = move |delta: i32| {
        let list = visible.get_untracked();
        if list.is_empty() {
            return;
        }
        let cur = selected.get_untracked();
        let at = cur
            .and_then(|c| list.iter().position(|r| r.key == c))
            .map(|i| i as i32)
            .unwrap_or(-1);
        let next = (at + delta).clamp(0, list.len() as i32 - 1) as usize;
        route::go(list[next].to.clone());
    };
    let new_button = match new_menu {
        Some(menu) => {
            Some(new_menu_button(menu, new_label.unwrap_or_default(), new_disabled).into_any())
        }
        None => new_button(on_new, new_label, new_disabled).map(IntoAny::into_any),
    };
    let chips = filter_chips(filters, filter, rows, no_counts);
    let list_ref: NodeRef<leptos::html::Div> = NodeRef::new();
    keep_selected_in_view(list_ref, selected, visible);
    let look = Look { no_counts, compact };
    let body = list_body(visible, rows, selected, empty, empty_icon, groups, look);
    view! {
        <aside
            class=move || format!("flex flex-col border-r border-border bg-surface lg:sticky lg:top-[var(--hdr-h,64px)] lg:h-[calc(100vh-var(--hdr-h,64px))]{}", if selected.get().is_some() { " max-lg:hidden" } else { "" })
            aria-label=title.clone()
            data-testid=(!testid.is_empty()).then_some(testid)
        >
            <div class="shrink-0 border-b border-border px-4 pt-4 pb-2.5" data-testid="list-head">
                <div class="mb-2.5 flex items-center gap-2">
                    <h2 class="text-md font-bold">{title.clone()}</h2>
                    <span class="text-sm text-text-muted" data-testid="list-count">{move || count.get()}</span>
                    {new_button}
                </div>
                <div class="relative mb-2.5">
                    <span class="pointer-events-none absolute top-1/2 left-2.5 -translate-y-1/2 text-text-faint"><Icon name="search" small=true/></span>
                    <input
                        id="list-search"
                        type="text"
                        class="h-8 w-full rounded-md border border-border-strong bg-input pr-8 pl-8 text-sm outline-none placeholder:text-text-faint focus:border-accent focus:ring-3 focus:ring-accent-weak"
                        placeholder=search_label.clone()
                        aria-label=search_label
                        prop:value=move || q.get()
                        on:input:target=move |e| q.set(e.target().value())
                    />
                    <span class="absolute top-1/2 right-2 -translate-y-1/2"><Kbd text="/"/></span>
                </div>
                <div class="flex flex-wrap gap-1.5">{chips}</div>
            </div>
            <div
                tabindex="0"
                class="relative min-h-0 flex-1 overflow-y-auto overscroll-contain pt-1 pb-4 outline-none [scrollbar-width:thin]"
                node_ref=list_ref
                data-testid="list"
                on:click=move |e| {
                    use wasm_bindgen::JsCast;
                    let Some(cb) = on_action else { return };
                    let hit = e
                        .target()
                        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                        .and_then(|t| t.closest("[data-action]").ok().flatten());
                    if let Some(el) = hit {
                        e.prevent_default();
                        cb.run((
                            el.get_attribute("data-key").unwrap_or_default(),
                            el.get_attribute("data-action").unwrap_or_default(),
                        ));
                    }
                }
                on:keydown=move |e| match e.key().as_str() {
                    "ArrowDown" => {
                        e.prevent_default();
                        move_sel(1);
                    }
                    "ArrowUp" => {
                        e.prevent_default();
                        move_sel(-1);
                    }
                    "Enter" => {
                        if let Some(el) = document().get_element_by_id("detail") {
                            use wasm_bindgen::JsCast;
                            if let Ok(el) = el.dyn_into::<web_sys::HtmlElement>() {
                                let _ = el.focus();
                            }
                        }
                    }
                    _ => {}
                }
            >
                {body}
            </div>
        </aside>
    }
}

#[derive(Clone, Copy)]
struct Look {
    no_counts: bool,
    compact: bool,
}

#[derive(Clone)]
pub struct NewChoice {
    pub key: &'static str,
    pub label: String,
    pub sub: String,
    pub on_pick: Callback<()>,
}

fn new_menu_button(menu: Vec<NewChoice>, label: String, disabled: bool) -> impl IntoView {
    let open = RwSignal::new(false);
    crate::app::popover(open, "new-menu-button", "new-menu");
    let items = menu
        .into_iter()
        .map(|c| {
            let pick = c.on_pick;
            view! {
                <li role="none">
                    <button
                        type="button"
                        role="menuitem"
                        class="flex w-full cursor-pointer flex-col items-start rounded-md px-3 py-2 text-left hover:bg-surface-3 focus-visible:bg-surface-3 focus-visible:outline-none"
                        data-new=c.key
                        on:click=move |_| {
                            open.set(false);
                            pick.run(());
                        }
                    >
                        <span class="text-sm font-semibold">{c.label}</span>
                        <span class="text-xs text-text-muted">{c.sub}</span>
                    </button>
                </li>
            }
        })
        .collect_view();
    view! {
        <span class="relative ml-auto">
            <button
                type="button"
                id="new-menu-button"
                class="flex size-[30px] cursor-pointer items-center justify-center rounded-md bg-accent text-on-accent hover:brightness-95 dark:hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-55"
                aria-label=label
                aria-haspopup="menu"
                aria-expanded=move || open.get().to_string()
                disabled=disabled
                title=disabled.then(|| needs_write_admin(WriteAction::Add, None))
                data-testid="new"
                on:click=move |_| open.update(|o| *o = !*o)
            >
                <Icon name="plus"/>
            </button>
            <ul
                role="menu"
                id="new-menu"
                tabindex="-1"
                class=burrow_ui_kit::menu_class("top-[36px] right-0 w-64", false)
                hidden=move || !open.get()
                data-testid="new-menu"
            >
                {items}
            </ul>
        </span>
    }
}

fn new_button(
    on_new: Option<Callback<()>>,
    new_label: Option<String>,
    new_disabled: bool,
) -> Option<impl IntoView> {
    on_new.zip(new_label).map(|(cb, label)| {
        view! {
            <button
                type="button"
                class="ml-auto flex size-[30px] cursor-pointer items-center justify-center rounded-md bg-accent text-on-accent hover:brightness-95 dark:hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-55"
                aria-label=label
                disabled=new_disabled
                title=new_disabled.then(|| needs_write_admin(WriteAction::Add, None))
                data-testid="new"
                on:click=move |_| cb.run(())
            >
                <Icon name="plus"/>
            </button>
        }
    })
}

fn filter_chips(
    filters: Vec<(&'static str, String)>,
    filter: RwSignal<&'static str>,
    rows: Signal<Vec<Row>>,
    no_counts: bool,
) -> impl IntoView {
    filters
        .into_iter()
        .map(|(key, label)| {
            view! {
                <button
                    type="button"
                    class="h-6 cursor-pointer rounded-full border px-2.5 text-xs whitespace-nowrap aria-pressed:border-accent-line aria-pressed:bg-accent-weak aria-pressed:text-accent-ink"
                    class:border-border=move || filter.get() != key
                    class:text-text-muted=move || filter.get() != key
                    aria-pressed=move || (filter.get() == key).to_string()
                    data-filter=key
                    on:click=move |_| filter.set(key)
                >
                    {label}
                    {move || {
                        let n = rows.with(|r| r.iter().filter(|x| !x.key.starts_with("__") && (key == "all" || x.filters.contains(&key))).count());
                        (!no_counts && key != "all" && n > 0 && Protocol::from_wire(key).is_none()).then(|| format!(" {n}"))
                    }}
                </button>
            }
        })
        .collect_view()
}

fn keep_selected_in_view(
    list_ref: NodeRef<leptos::html::Div>,
    selected: Signal<Option<String>>,
    visible: Memo<Vec<Row>>,
) {
    Effect::new(move |_| {
        selected.track();
        visible.track();
        request_animation_frame(move || {
            use wasm_bindgen::JsCast;
            let Some(list) = list_ref.try_get_untracked().flatten() else {
                return;
            };
            let list: web_sys::HtmlElement = list.into();
            let Some(el) = list
                .query_selector("[data-selected=true]")
                .ok()
                .flatten()
                .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
            else {
                return;
            };
            let (top, h) = (el.offset_top(), el.offset_height());
            let (st, ch) = (list.scroll_top(), list.client_height());
            if top < st {
                list.set_scroll_top((top - 8).max(0));
            } else if top + h > st + ch {
                list.set_scroll_top(top + h - ch + 8);
            }
        });
    });
}

fn list_body(
    visible: Memo<Vec<Row>>,
    rows: Signal<Vec<Row>>,
    selected: Signal<Option<String>>,
    empty: Option<(String, String)>,
    empty_icon: Option<&'static str>,
    groups: Vec<&'static str>,
    look: Look,
) -> impl Fn() -> AnyView {
    let Look { no_counts, compact } = look;
    move || {
        let list = visible.get();
        if list.is_empty() {
            return match &empty {
                Some((t, _)) if t.is_empty() => ().into_any(),
                Some((t, m)) if rows.with(Vec::is_empty) => view! { <EmptyState icon=empty_icon.unwrap_or("plus") title=t.clone() message=m.clone() testid="list-empty"/> }.into_any(),
                _ => view! { <p class="px-4 py-6 text-sm text-text-muted">"Nothing matches."</p> }.into_any(),
            };
        }
        let sel = selected.get();
        let mut gs: Vec<String> = groups.iter().map(|g| g.to_string()).collect();
        for r in &list {
            if !gs.contains(&r.group) {
                gs.push(r.group.clone());
            }
        }
        let shown = gs
            .iter()
            .filter(|g| list.iter().any(|r| &r.group == *g))
            .count();
        gs.into_iter()
            .filter_map(|g| {
                let in_group: Vec<Row> = list.iter().filter(|r| r.group == g).cloned().collect();
                (!in_group.is_empty()).then(|| {
                    let n = in_group.len();
                    view! {
                        {(!g.is_empty()).then(|| view! {
                            <div class=format!("flex justify-between px-4 pt-3.5 pb-1.5 {LABEL}")>
                                <span>{g.clone()}</span>
                                {(shown > 1 && !no_counts).then(|| view! { <span>{n}</span> })}
                            </div>
                        })}
                        {in_group.into_iter().map(|r| {
                            let on = sel.as_deref() == Some(r.key.as_str());
                            if compact {
                                view! { <TocRow row=r selected=on/> }.into_any()
                            } else {
                                view! { <ListCard row=r selected=on/> }.into_any()
                            }
                        }).collect_view()}
                    }
                })
            })
            .collect_view()
            .into_any()
    }
}
