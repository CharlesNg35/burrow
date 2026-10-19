mod about;
pub mod activity;
pub mod create;
mod entity;
pub mod flow;
mod overview;
pub mod paths;
mod settings;

use burrow_ui_kit::{EmptyState, Icon, Kbd, LABEL, Tone, button_class, tag_class};
use leptos::prelude::*;

use crate::route::{self, Link, Route};
use crate::store::store;

pub fn screen(route: Route) -> AnyView {
    let _ = store();
    match route {
        Route::Overview => overview::Overview().into_any(),
        Route::List(section) => view! { <entity::Entities section=section selected=None/> }.into_any(),
        Route::Detail(section, name) => view! { <entity::Entities section=section selected=Some(name)/> }.into_any(),
        Route::Settings => settings::Settings().into_any(),
        Route::Activity(rev) => view! { <activity::Activity selected=rev/> }.into_any(),
        Route::Review => crate::review::ReviewPage().into_any(),
        Route::About => about::About().into_any(),
        Route::NotFound(path) => view! {
            <div class="p-8">
                <EmptyState title="Page not found" message=format!("Nothing lives at {path}.") testid="not-found">
                    <Link to=Route::Overview class=button_class("secondary", false)>"Go to the overview"</Link>
                </EmptyState>
            </div>
        }
        .into_any(),
    }
}

pub fn wide() -> bool {
    window()
        .inner_width()
        .ok()
        .and_then(|w| w.as_f64())
        .unwrap_or(1440.0)
        >= 1024.0
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub key: String,
    pub to: Route,
    pub icon: &'static str,
    pub dot: Option<Tone>,
    pub title: String,
    pub meta: String,
    pub line2: String,
    pub tags: Vec<(String, Tone)>,
    pub group: String,
    pub pending: Option<&'static str>,
    pub filters: Vec<&'static str>,
    pub actions: Vec<&'static str>,
}

impl Row {
    fn matches(&self, q: &str) -> bool {
        q.is_empty()
            || self.title.to_lowercase().contains(q)
            || self.line2.to_lowercase().contains(q)
            || self.meta.to_lowercase().contains(q)
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
                <Link to=row.to.clone() class="block truncate font-semibold text-text no-underline after:absolute after:inset-0 after:rounded-[11px]" testid="list-row" current=Signal::stored(selected)>
                    {row.title.clone()}
                </Link>
                <span class="block truncate text-sm text-text-muted">{row.line2.clone()}</span>
                {(!row.tags.is_empty() || row.pending.is_some()).then(|| view! {
                    <span class="mt-1.5 flex flex-wrap gap-1.5">
                        {row.pending.map(|p| view! { <span class=tag_class(Tone::Accent) data-pending=p>{p}</span> })}
                        {row.tags.iter().map(|(t, tone)| view! { <span class=tag_class(*tone)>{t.clone()}</span> }).collect_view()}
                    </span>
                })}
                {(!row.actions.is_empty()).then(|| view! {
                    <span class="relative z-10 mt-1.5 flex justify-end gap-3 text-xs">
                        {row.actions.iter().map(|a| view! {
                            <button type="button" class="cursor-pointer font-semibold text-accent-ink hover:underline" data-action=a.to_lowercase() data-key=key.clone()>{*a}</button>
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
) -> impl IntoView {
    let q = RwSignal::new(String::new());
    let filter = RwSignal::new("all");
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
    let new_button = on_new.zip(new_label).map(|(cb, label)| {
        view! {
            <button type="button" class="ml-auto flex size-[30px] cursor-pointer items-center justify-center rounded-md bg-accent text-on-accent hover:brightness-110" aria-label=label data-testid="new" on:click=move |_| cb.run(())>
                <Icon name="plus"/>
            </button>
        }
    });
    let chips = filters
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
                        let n = rows.with(|r| r.iter().filter(|x| key == "all" || x.filters.contains(&key)).count());
                        (n > 0 && key != "tcp" && key != "udp").then(|| format!(" {n}"))
                    }}
                </button>
            }
        })
        .collect_view();
    let body = move || {
        let list = visible.get();
        if list.is_empty() {
            return match &empty {
                Some((t, m)) if rows.with(Vec::is_empty) => view! { <EmptyState icon="plus" title=t.clone() message=m.clone() testid="list-empty"/> }.into_any(),
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
        gs.into_iter()
            .filter_map(|g| {
                let in_group: Vec<Row> = list.iter().filter(|r| r.group == g).cloned().collect();
                (!in_group.is_empty()).then(|| {
                    let n = in_group.len();
                    view! {
                        {(!g.is_empty()).then(|| view! {
                            <div class=format!("flex justify-between px-4 pt-3.5 pb-1.5 {LABEL}")>
                                <span>{g.clone()}</span>
                                <span>{n}</span>
                            </div>
                        })}
                        {in_group.into_iter().map(|r| {
                            let on = sel.as_deref() == Some(r.key.as_str());
                            view! { <ListCard row=r selected=on/> }
                        }).collect_view()}
                    }
                })
            })
            .collect_view()
            .into_any()
    };
    view! {
        <aside
            class=move || format!("border-r border-border bg-surface pb-4 lg:min-h-[calc(100vh-64px)]{}", if selected.get().is_some() { " max-lg:hidden" } else { "" })
            aria-label=title.clone()
            data-testid=(!testid.is_empty()).then_some(testid)
        >
            <div class="border-b border-border px-4 pt-4 pb-2.5">
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
                class="pt-1 outline-none"
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

#[component]
pub fn DetailPane(
    #[prop(optional, into)] back: Option<Route>,
    children: Children,
) -> impl IntoView {
    view! {
        <section id="detail" tabindex="-1" class="min-w-0 bg-bg px-[34px] pt-7 pb-12 outline-none max-sm:px-4" data-testid="detail-pane">
            {back.map(|b| view! {
                <div class="mb-3 lg:hidden">
                    <Link to=b class=button_class("ghost", true)>
                        <Icon name="chevron-left" small=true/>
                        "Back"
                    </Link>
                </div>
            })}
            <div class="max-w-[1120px]">{children()}</div>
        </section>
    }
}

pub fn master_detail(list: AnyView, detail: AnyView) -> AnyView {
    view! {
        <div class="grid min-h-[calc(100vh-64px)] lg:grid-cols-[380px_minmax(0,1fr)]">
            {list}
            {detail}
        </div>
    }
    .into_any()
}

pub fn no_token() -> impl IntoView {
    let port = window().location().port().unwrap_or_default();
    view! {
        <div class="flex min-h-screen items-center justify-center bg-bg p-6" data-testid="no-token">
            <div class="grid w-full max-w-[860px] gap-8 rounded-[16px] border border-border bg-surface p-8 md:grid-cols-2">
                <div>
                    <div class="mb-6 flex items-center gap-2.5">
                        <span class="flex size-[30px] items-center justify-center rounded-md bg-gradient-to-b from-logo-from to-logo-to text-white" aria-hidden="true"><Icon name="logo" large=true/></span>
                        <span class="font-semibold">"burrow admin"</span>
                    </div>
                    <h1 class="mb-2 text-xl font-bold">"Open the link from your terminal"</h1>
                    <p class="mb-4 text-sm text-text-muted">"This page needs the one-time link that burrow admin printed. The link keeps other programs on this machine from using your admin session."</p>
                    <ol class="list-decimal space-y-1.5 pl-5 text-sm">
                        <li>"Go back to the terminal where you ran burrow admin."</li>
                        <li>"Open the link it shows, or run it again with --print-url."</li>
                        <li>"Keep the terminal open while you use this page."</li>
                    </ol>
                </div>
                <pre class="overflow-x-auto rounded-[10px] bg-code p-4 font-mono text-xs leading-relaxed text-code-text">
                    "$ burrow admin --print-url\nburrow admin for your server listens on\n"
                    <span class="text-accent">{format!("http://127.0.0.1:{port}/#token=…")}</span>
                    "\nstop it with Ctrl-C"
                </pre>
                <p class="text-xs text-text-muted md:col-span-2">{format!("Served by burrow admin on 127.0.0.1:{port} · nothing was sent to the server.")}</p>
            </div>
        </div>
    }
}
