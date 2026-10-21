use burrow_admin_proto::msg::{AdminLevel, FileDiagnostics, UiDiagnostics};
use burrow_admin_proto::schema::Entity;
use burrow_admin_proto::serde_json::{self, Value};
use burrow_config::admin_schema::AdminEnum;
use burrow_config::{
    ClientEntry, Entry, ServerConfig, ServiceEntry, VisitorEntry, entity_of_section,
};
use burrow_ui_kit::{Banner, Button, EmptyState, Icon, Kbd, Skeleton, ToastHost, Tone, copy_text};
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;

use crate::api;
use crate::format;
use crate::review::ConflictDialog;
use crate::route::{self, Link, Route};
use crate::screens;
use crate::store::{Conn, Store, short_rev, store};

const THEME_KEY: &str = "burrow-admin-theme";

fn apply_theme(theme: &str) {
    if let Some(root) = document().document_element() {
        let list = root.class_list();
        let _ = list.remove_2("light", "dark");
        if theme == "light" || theme == "dark" {
            let _ = list.add_1(theme);
        }
    }
}

fn saved_theme() -> String {
    window()
        .local_storage()
        .ok()
        .flatten()
        .and_then(|s| s.get_item(THEME_KEY).ok().flatten())
        .unwrap_or_default()
}

fn typing_now() -> bool {
    document()
        .active_element()
        .map(|e| {
            let tag = e.tag_name().to_lowercase();
            tag == "input"
                || tag == "textarea"
                || tag == "select"
                || e.has_attribute("contenteditable")
        })
        .unwrap_or(false)
}

pub fn diagnostics(s: Store) -> String {
    let w = s.welcome.get_untracked();
    let st = s.state.get_untracked();
    let schema = s.schema.get_untracked();
    serde_json::to_string(&UiDiagnostics {
        ui: env!("CARGO_PKG_VERSION").to_string(),
        server: w.as_ref().map(|w| w.server.version.clone()),
        target: w.as_ref().map(|w| w.server.target.clone()),
        protocol: w.as_ref().map(|w| w.protocol),
        features: w.as_ref().map(|w| w.features.clone()),
        schema_hash: schema.as_ref().map(|s| s.hash.clone()),
        schema_version: schema.as_ref().map(|s| s.schema_version),
        file: st.as_ref().map(|s| FileDiagnostics {
            format: s.file.format.clone(),
            writable: s.file.writable,
            running_ok: s.running.ok,
        }),
    })
    .unwrap_or_default()
}

#[component]
pub fn App() -> impl IntoView {
    let s = Store::new(route::current());
    provide_context(s);
    apply_theme(&saved_theme());
    let on_pop = Closure::<dyn FnMut()>::new(move || s.route.set(route::current()));
    window().set_onpopstate(Some(on_pop.as_ref().unchecked_ref()));
    on_pop.forget();
    let on_hash = Closure::<dyn FnMut()>::new(move || {
        if s.conn.get_untracked() == Conn::NoToken {
            let header_height = || {
                if let Some(h) = document()
                    .query_selector("[data-testid=header]")
                    .ok()
                    .flatten()
                    && let Some(root) = document().document_element()
                {
                    let px = h.get_bounding_client_rect().height();
                    let _ = root
                        .unchecked_ref::<web_sys::HtmlElement>()
                        .style()
                        .set_property("--hdr-h", &format!("{px}px"));
                }
            };
            let resize = Closure::<dyn FnMut()>::new(header_height);
            let _ = window()
                .add_event_listener_with_callback("resize", resize.as_ref().unchecked_ref());
            resize.forget();
            set_timeout(header_height, std::time::Duration::from_millis(100));
            set_timeout(header_height, std::time::Duration::from_millis(1500));
            api::connect(s);
        }
    });
    window().set_onhashchange(Some(on_hash.as_ref().unchecked_ref()));
    on_hash.forget();
    let on_leave = Closure::<dyn FnMut(web_sys::BeforeUnloadEvent)>::new(
        move |ev: web_sys::BeforeUnloadEvent| {
            if s.edit.dirty() {
                ev.prevent_default();
                ev.set_return_value("You have changes that are not applied.");
            }
        },
    );
    window().set_onbeforeunload(Some(on_leave.as_ref().unchecked_ref()));
    on_leave.forget();
    let on_key =
        Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |e: web_sys::KeyboardEvent| {
            if e.key() == "Escape" && s.search_open.get_untracked() {
                s.search_open.set(false);
                return;
            }
            if e.ctrl_key() || e.meta_key() || e.alt_key() || typing_now() {
                return;
            }
            match e.key().as_str() {
                "f" | "F" if s.conn.get_untracked() != Conn::NoToken => {
                    e.prevent_default();
                    s.flow_open.set(true);
                }
                "/" => {
                    e.prevent_default();
                    match document().get_element_by_id("list-search") {
                        Some(el) => {
                            if let Ok(el) = el.dyn_into::<web_sys::HtmlElement>() {
                                let _ = el.focus();
                            }
                        }
                        None => s.search_open.set(true),
                    }
                }
                _ => {}
            }
        });
    let _ = document().add_event_listener_with_callback("keydown", on_key.as_ref().unchecked_ref());
    on_key.forget();
    let _ = set_interval_with_handle(
        move || s.now.set(js_sys::Date::now()),
        std::time::Duration::from_millis(1000),
    );
    api::connect(s);
    let no_token = Memo::new(move |_| s.conn.get() == Conn::NoToken);
    view! {
        <a class="sr-only focus:not-sr-only focus:fixed focus:top-2 focus:left-2 focus:z-50 focus:rounded-md focus:bg-surface focus:p-2" href="#main">"Skip to content"</a>
        {move || {
            if no_token.get() {
                screens::no_token().into_any()
            } else {
                view! {
                    <Header/>
                    <Banners/>
                    <main id="main" tabindex="-1" class="min-h-[calc(100vh-64px)]">
                        <Body/>
                    </main>
                    <ConflictDialog/>
                    <screens::flow::FlowModal/>
                    <SearchPopover/>
                }
                .into_any()
            }
        }}
        <ToastHost
            toasts=s.toasts
            on_action=move |_| route::go(Route::Review)
            on_hover=move |h: bool| s.toast_hold.set(h)
        />
    }
}

fn initials(name: &str) -> String {
    let mut parts = name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|p| !p.is_empty());
    let a = parts.next().and_then(|p| p.chars().next());
    let b = parts
        .next()
        .and_then(|p| p.chars().next())
        .or_else(|| name.chars().nth(1));
    [a, b]
        .into_iter()
        .flatten()
        .collect::<String>()
        .to_uppercase()
}

pub fn host_line(s: Store) -> String {
    let public = s.state.with(|st| {
        st.as_ref().and_then(|st| {
            ServerConfig::PUBLIC_ADDRESS
                .str(&st.values)
                .map(str::to_string)
        })
    });
    public
        .or_else(|| {
            s.status
                .with(|st| st.as_ref().and_then(|st| st.listening.first().cloned()))
        })
        .unwrap_or_default()
}

#[component]
fn Header() -> impl IntoView {
    let s = store();
    let tabs: [(Route, &str, &str); 6] = [
        (Route::Overview, "Overview", "overview"),
        (
            Route::List(ServiceEntry::section().into()),
            "Services",
            "stack",
        ),
        (
            Route::List(ClientEntry::section().into()),
            "Clients",
            "monitor",
        ),
        (
            Route::List(VisitorEntry::section().into()),
            "Visitors",
            "person",
        ),
        (Route::Settings, "Settings", "settings"),
        (Route::Activity(None), "Activity", "activity"),
    ];
    let nav = tabs
        .into_iter()
        .map(|(to, label, icon)| {
            let key = to.section().to_string();
            let current = move || s.route.with(|r| r.section() == key);
            view! {
                <Link to=to class="flex h-[34px] items-center gap-1.5 rounded-md px-3 text-[13px] font-semibold text-hdr-muted no-underline hover:text-hdr-on aria-[current=page]:bg-tab-on aria-[current=page]:text-hdr-on aria-[current=page]:shadow-sm aria-[current=page]:ring-1 aria-[current=page]:ring-border aria-[current=page]:ring-inset max-xl:px-2.5 max-sm:h-8 max-sm:shrink-0 max-sm:aria-[current=page]:bg-accent-weak max-sm:aria-[current=page]:text-accent-ink max-sm:aria-[current=page]:shadow-none max-sm:aria-[current=page]:ring-0 dark:aria-[current=page]:ring-border-strong" testid="nav" current=Signal::derive(current.clone())>
                    <span class=move || if current() { "text-accent max-md:hidden" } else { "max-md:hidden" }><Icon name=icon/></span>
                    <span>{label}</span>
                </Link>
            }
        })
        .collect_view();
    let menu_open = RwSignal::new(false);
    let avatar_open = RwSignal::new(false);
    let theme = RwSignal::new(saved_theme());
    let set_theme = move |t: &'static str| {
        theme.set(t.to_string());
        apply_theme(t);
        if let Some(st) = window().local_storage().ok().flatten() {
            let _ = st.set_item(THEME_KEY, t);
        }
    };
    let pending = Memo::new(move |_| {
        if s.route.with(|r| *r == Route::Review) {
            s.edit.review_count()
        } else {
            s.edit.pending()
        }
    });
    let check_state = Signal::derive(move || {
        let e = s.edit;
        let errors = e.local.with(|l| l.len())
            + e.related.with(|r| r.len())
            + e.plan.with(|p| p.as_ref().map(|p| p.errors()).unwrap_or(0));
        if e.checking.get() {
            "checking".to_string()
        } else if errors > 0 {
            "errors".to_string()
        } else {
            "ok".to_string()
        }
    });
    let read_only = Memo::new(move |_| s.conn.get() == Conn::Open && !s.can_write());
    let why_open = RwSignal::new(false);
    let retry_open = RwSignal::new(false);
    popover(menu_open, "server-switcher", "server-menu");
    popover(avatar_open, "avatar", "avatar-menu");
    popover(why_open, "read-only-tag", "read-only-why");
    popover(retry_open, "conn-pill", "retry-popover");
    let conn_state = move || {
        let conn = s.conn.get();
        match conn {
            Conn::Reconnecting { .. } => {
                let left = ((s.retry_at.get() - s.now.get()) / 1000.0).ceil().max(0.0) as u64;
                view! {
                    <span class="relative">
                        <button type="button" class="flex cursor-pointer items-center gap-1.5 text-[12.5px] text-warning" aria-haspopup="true" aria-expanded=move || retry_open.get().to_string() aria-controls="retry-popover" id="conn-pill" data-testid="conn-pill" on:click=move |_| retry_open.update(|o| *o = !*o)>
                            <Icon name="refresh"/>
                            {format!("Reconnecting in {left} s")}
                        </button>
                        <Show when=move || retry_open.get()>
                            <div class="absolute top-8 right-0 z-40 w-56 rounded-[10px] border border-border-strong bg-surface p-3 text-sm text-text shadow-overlay outline-none dark:bg-surface-2" id="retry-popover" tabindex="-1" role="dialog" aria-label="Connection" data-testid="retry-popover">
                                <p class="mb-2 text-text-muted">"The connection to burrow admin dropped. It retries on its own."</p>
                                <Button label="Retry now" small=true testid="retry-now" on_click=move |_| { retry_open.set(false); api::reconnect(s); }/>
                            </div>
                        </Show>
                    </span>
                }
                .into_any()
            }
            Conn::Open => view! { <span class="sr-only" data-testid="conn-pill">{move || format!("Connected to {}", host_line(s))}</span> }.into_any(),
            Conn::Connecting => view! { <span class="text-[12.5px] text-hdr-muted" data-testid="conn-pill">"Connecting"</span> }.into_any(),
            other => view! { <span class="flex items-center gap-1.5 text-[12.5px] text-danger" data-testid="conn-pill"><Icon name="wifi-off"/>{other.label()}</span> }.into_any(),
        }
    };
    let dot = move || match s.conn.get() {
        Conn::Open if read_only.get() => "bg-text-faint",
        Conn::Open => "bg-success",
        Conn::Reconnecting { .. } | Conn::Connecting => "bg-warning",
        _ => "bg-danger",
    };
    let conn_line = move || match s.conn.get() {
        Conn::Open => format!("Connected to {}", host_line(s)),
        Conn::Reconnecting { .. } => format!("Reconnecting to {}", host_line(s)),
        other => other.label().to_string(),
    };
    let next_theme = move || match theme.get().as_str() {
        "" => ("System", "Light", "light"),
        "light" => ("Light", "Dark", "dark"),
        _ => ("Dark", "System", ""),
    };
    let you = Memo::new(move |_| {
        s.welcome.with(|w| {
            w.as_ref()
                .map(|w| (w.you.visitor.clone(), w.you.level.wire()))
        })
    });
    let facts = move || {
        let w = s.welcome.get()?;
        let st = s.state.get()?;
        Some(view! {
            <dl class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 px-3 py-2 text-sm [&_dt]:text-text-muted [&_dd]:font-mono [&_dd]:text-xs [&_dd]:break-all">
                <dt>"Version"</dt><dd>{w.server.version.clone()}</dd>
                <dt>"Up"</dt><dd>{format::span(s.status.get().map(|x| x.uptime_secs).unwrap_or(w.server.uptime_secs))}</dd>
                <dt>"Revision"</dt><dd>{short_rev(&st.rev)}</dd>
                <dt>"Config"</dt><dd>{st.file.path.clone()}</dd>
            </dl>
        })
    };
    view! {
        <header class="sticky top-0 z-30 grid grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] items-center gap-4 border-b border-hdr-line bg-hdr px-5 text-hdr-text max-xl:grid-cols-[minmax(0,1fr)_auto] max-xl:pt-3 max-sm:gap-x-2 max-sm:px-3 max-sm:pt-2 xl:h-16" data-testid="header">
            {move || match s.route.get() {
                Route::Detail(sec, name) => {
                    let label = match entity_of_section(&sec) {
                        Some(Entity::Client) => "Clients",
                        Some(Entity::Visitor) => "Visitors",
                        _ => "Services",
                    };
                    Some(view! {
                        <div class="flex min-w-0 items-center gap-2 sm:hidden" data-testid="mobile-back">
                            <Link to=Route::List(sec.clone()) class="flex size-8 items-center justify-center rounded-md text-accent-ink no-underline">
                                <Icon name="chevron-left"/>
                                <span class="sr-only">{format!("Back to {label}")}</span>
                            </Link>
                            <span class="flex min-w-0 flex-col">
                                <span class="truncate font-semibold text-hdr-on">{name}</span>
                                <span class="truncate text-[11px] text-hdr-muted">{move || if s.editing.get() { format!("{label} · editing") } else { label.to_string() }}</span>
                            </span>
                        </div>
                    })
                }
                _ => None,
            }}
            <div class=move || format!("relative flex min-w-0 items-center gap-2.5{}", if matches!(s.route.get(), Route::Detail(..)) { " max-sm:hidden" } else { "" })>
                <span class="flex size-[30px] shrink-0 items-center justify-center rounded-md bg-gradient-to-b from-logo-from to-logo-to text-white" aria-hidden="true"><Icon name="logo" large=true/></span>
                <button type="button" class="flex min-w-0 cursor-pointer items-center gap-2 rounded-md px-1.5 py-1 hover:bg-hdr-raised" aria-haspopup="true" aria-expanded=move || menu_open.get().to_string() aria-controls="server-menu" id="server-switcher" data-testid="server-switcher" on:click=move |_| menu_open.update(|o| *o = !*o)>
                    <span class="flex min-w-0 flex-col text-left">
                        <span class="truncate font-semibold text-hdr-on" data-testid="server-line">{move || s.server_name()}</span>
                        <span class="truncate text-[11px] text-hdr-muted sm:hidden">{move || {
                            let v = s.welcome.with(|w| w.as_ref().map(|w| w.server.version.clone()).unwrap_or_default());
                            let up = s.status.with(|st| st.as_ref().map(|st| crate::format::span(st.uptime_secs)));
                            match up {
                                Some(u) => format!("v{v} · up {u}"),
                                None => format!("v{v}"),
                            }
                        }}</span>
                    </span>
                    {move || {
                        let host = host_line(s);
                        (host != s.server_name()).then(|| view! { <span class="truncate font-mono text-xs text-hdr-muted max-sm:hidden">{host}</span> })
                    }}
                    <Icon name="chevron-down" small=true/>
                </button>
                <Show when=move || menu_open.get()>
                    <div class="absolute top-11 left-10 z-40 w-80 rounded-[10px] border border-border-strong bg-surface p-1 text-text shadow-overlay outline-none dark:bg-surface-2" id="server-menu" tabindex="-1" data-testid="server-menu" on:click=move |_| menu_open.set(false)>
                        {facts}
                        <div class="border-t border-border p-1">
                            <Link to=Route::About class="flex items-center gap-2 rounded-md px-2 py-1.5 text-sm no-underline hover:bg-surface-3">"About this server"</Link>
                        </div>
                    </div>
                </Show>
            </div>
            <nav aria-label="Sections" class="flex gap-0.5 rounded-[11px] border border-hdr-line bg-hdr-raised p-[3px] max-xl:col-span-2 max-xl:row-start-2 max-xl:mb-3 max-xl:justify-self-center max-sm:mb-2 max-sm:max-w-full max-sm:justify-self-stretch max-sm:overflow-x-auto max-sm:rounded-none max-sm:border-0 max-sm:bg-transparent max-sm:p-0 max-sm:[scrollbar-width:none] dark:border-hdr-raised-2">
                {nav}
            </nav>
            <div class="flex items-center justify-end gap-3" data-testid="header-right">
                <button type="button" class="flex size-8 cursor-pointer items-center justify-center rounded-lg text-hdr-text outline-none hover:bg-hdr-raised focus-visible:bg-hdr-raised max-sm:hidden" aria-label="Search (/)" title="Search (/)" data-testid="search-open" on:click=move |_| s.search_open.set(true)>
                    <Icon name="search"/>
                </button>
                <button
                    type="button"
                    class="flex size-8 cursor-pointer items-center justify-center rounded-lg text-hdr-text outline-none hover:bg-hdr-raised focus-visible:bg-hdr-raised max-[1199px]:hidden"
                    aria-label=move || { let (now, next, _) = next_theme(); format!("Theme: {now}. Switch to {next}") }
                    title=move || { let (now, next, _) = next_theme(); format!("Theme: {now}. Switch to {next}") }
                    data-testid="theme-toggle"
                    data-theme=move || if theme.get().is_empty() { "system".to_string() } else { theme.get() }
                    on:click=move |_| { let (_, _, t) = next_theme(); set_theme(t); }
                >
                    {move || match theme.get().as_str() {
                        "light" => view! { <Icon name="sun"/> }.into_any(),
                        "dark" => view! { <Icon name="moon"/> }.into_any(),
                        _ => view! { <Icon name="monitor"/> }.into_any(),
                    }}
                </button>
                {conn_state}
                {move || {
                    if read_only.get() {
                        view! {
                            <span class="relative">
                                <button type="button" class="inline-flex h-8 cursor-pointer items-center gap-1.5 rounded-lg border border-hdr-line bg-hdr-raised px-2.5 text-[12.5px] font-semibold text-hdr-text dark:border-hdr-raised-2" aria-haspopup="true" aria-expanded=move || why_open.get().to_string() aria-controls="read-only-why" id="read-only-tag" data-testid="read-only-tag" on:click=move |_| why_open.update(|o| *o = !*o)>
                                    <Icon name="eye" small=true/>
                                    "Read-only"
                                </button>
                                <Show when=move || why_open.get()>
                                    <div class="absolute top-8 right-0 z-40 w-80 rounded-[10px] border border-border-strong bg-surface p-3 text-sm text-text shadow-overlay outline-none dark:bg-surface-2" id="read-only-why" tabindex="-1" role="dialog" aria-label="Why this is read-only" data-testid="read-only-why">
                                        {move || {
                                            let (title, hint) = read_only_reason(s);
                                            view! {
                                                <p class="mb-1.5 flex items-center gap-2 font-semibold"><Icon name="eye" small=true/>"Read-only"</p>
                                                <p class="text-text-muted">{title}</p>
                                                {(!hint.is_empty()).then(|| view! { <p class="mt-1 font-mono text-xs break-all text-text-muted">{hint}</p> })}
                                            }
                                        }}
                                    </div>
                                </Show>
                            </span>
                        }
                        .into_any()
                    } else if pending.get() > 0 {
                        view! {
                            <Link to=Route::Review class="inline-flex h-8 items-center gap-1.5 rounded-md bg-accent px-3 text-sm font-semibold text-on-accent no-underline hover:brightness-95 dark:hover:brightness-110" testid="review-open" state=check_state>
                                <Icon name="diff"/>
                                <span class="max-sm:sr-only">"Review"</span>
                                <span class="rounded-sm bg-white/25 px-1.5 text-[11.5px] dark:bg-black/18" data-testid="review-count">{pending.get()}</span>
                            </Link>
                        }
                        .into_any()
                    } else {
                        ().into_any()
                    }
                }}
                <div class="relative flex items-center gap-2 max-sm:hidden">
                    <button type="button" class="relative flex size-[30px] cursor-pointer items-center justify-center rounded-full bg-hdr-raised-2 text-xs font-bold text-hdr-on" aria-label=move || format!("Account {}", you.get().map(|y| y.0).unwrap_or_default()) title=conn_line aria-haspopup="true" aria-expanded=move || avatar_open.get().to_string() aria-controls="avatar-menu" id="avatar" data-testid="avatar" on:click=move |_| avatar_open.update(|o| *o = !*o)>
                        {move || you.get().map(|y| initials(&y.0)).unwrap_or_default()}
                        <span class=move || format!("absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full ring-2 ring-hdr {}", dot())></span>
                    </button>
                    <span class="sr-only" data-testid="you-level">{move || you.get().map(|y| format!("{} ({})", y.0, y.1)).unwrap_or_default()}</span>
                    <Show when=move || avatar_open.get()>
                        <div class="absolute top-10 right-0 z-40 w-64 rounded-[10px] border border-border-strong bg-surface p-1 text-text shadow-overlay outline-none dark:bg-surface-2" id="avatar-menu" tabindex="-1" data-testid="avatar-menu">
                            <div class="px-2.5 pt-2 pb-2 text-sm" data-testid="avatar-who">
                                <span class="font-semibold">{move || you.get().map(|y| y.0).unwrap_or_default()}</span>
                                <span class="text-text-muted">{move || you.get().map(|y| format!(" · admin {}", y.1)).unwrap_or_default()}</span>
                            </div>
                            <div class="flex items-center gap-2 border-t border-border px-2.5 py-2 text-sm" data-testid="avatar-conn">
                                <span class=move || format!("size-2 shrink-0 rounded-full {}", dot())></span>
                                <span class="truncate">{conn_line}</span>
                            </div>
                            <div class="flex items-center gap-2 border-t border-border px-2 py-2">
                                <span class="text-sm">"Theme"</span>
                                <span class="ml-auto flex rounded-md bg-surface-3 p-0.5" role="group" aria-label="Theme">
                                    {[("", "System", "monitor"), ("light", "Light", "sun"), ("dark", "Dark", "moon")]
                                        .into_iter()
                                        .map(|(t, l, icon)| view! {
                                            <button
                                                type="button"
                                                class=move || format!("flex cursor-pointer items-center gap-1 rounded-[5px] px-2 py-1 text-xs {}", if theme.get() == t { "bg-surface font-semibold text-text shadow-sm dark:bg-surface-2" } else { "text-text-muted" })
                                                aria-pressed=move || (theme.get() == t).to_string()
                                                data-theme-choice=if t.is_empty() { "system" } else { t }
                                                on:click=move |_| { set_theme(t); avatar_open.set(false); }
                                            >
                                                <Icon name=icon small=true/>
                                                {l}
                                            </button>
                                        })
                                        .collect_view()}
                                </span>
                            </div>
                            <div class="border-t border-border pt-1">
                                <button type="button" class="flex w-full cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-surface-3" data-testid="copy-diagnostics" on:click=move |_| {
                                    copy_text(&diagnostics(s));
                                    avatar_open.set(false);
                                    s.toast(Tone::Success, "Copied diagnostics", None);
                                }>
                                    <Icon name="copy" small=true/>
                                    "Copy diagnostics"
                                </button>
                                <Link to=Route::About class="flex items-center gap-2 rounded-md px-2 py-1.5 text-sm text-text no-underline hover:bg-surface-3" testid="avatar-about">
                                    <Icon name="about" small=true/>
                                    "About"
                                </Link>
                            </div>
                        </div>
                    </Show>
                </div>
            </div>
        </header>
    }
}

#[component]
fn Body() -> impl IntoView {
    let s = store();
    let conn = Memo::new(move |_| s.conn.get());
    let loaded =
        Memo::new(move |_| s.state.with(Option::is_some) && s.schema.with(Option::is_some));
    let route = Memo::new(move |_| s.route.get());
    let dim = Memo::new(move |_| {
        matches!(
            s.conn.get(),
            Conn::Reconnecting { .. } | Conn::Closed { .. }
        )
    });
    move || {
        let c = conn.get();
        match c {
            Conn::Refused { reason, message } => view! {
                <div class="mx-auto max-w-2xl p-8" data-testid="refused" data-reason=reason.clone()>
                    <EmptyState icon="wifi-off" title="The server refused the admin session" message=message>
                        <p class="text-sm text-text-muted">"Run burrow admin again in your terminal to open a new session."</p>
                    </EmptyState>
                </div>
            }
            .into_any(),
            Conn::Revoked { reason } => view! {
                <div class="mx-auto max-w-2xl p-8" data-testid="revoked">
                    <EmptyState icon="lock" title="Your admin access was removed" message=format!("The server closed this session: {reason}")>
                        <p class="text-sm text-text-muted">"Ask a write admin, then run burrow admin again."</p>
                    </EmptyState>
                </div>
            }
            .into_any(),
            Conn::Closed { message } if !loaded.get() => view! {
                <div class="mx-auto max-w-2xl p-8" data-testid="closed">
                    <EmptyState icon="wifi-off" title="Disconnected" message=message>
                        <Button label="Reconnect" variant="primary" on_click=move |_| api::reconnect(s)/>
                    </EmptyState>
                </div>
            }
            .into_any(),
            _ if !loaded.get() => view! { <div data-testid="loading" class="p-8"><Skeleton lines=6/></div> }.into_any(),
            _ => view! {
                <div class=move || if dim.get() { "opacity-45 grayscale-50" } else { "" } data-dimmed=move || dim.get().then_some("true")>
                    {move || screens::screen(route.get())}
                </div>
            }
            .into_any(),
        }
    }
}

fn html_by_id(id: &str) -> Option<web_sys::HtmlElement> {
    document()
        .get_element_by_id(id)
        .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
}

pub fn popover(open: RwSignal<bool>, trigger: &'static str, panel: &'static str) {
    Effect::new(move |was: Option<bool>| {
        let now = open.get();
        if now && was != Some(true) {
            set_timeout(
                move || {
                    if let Some(el) = html_by_id(panel) {
                        burrow_ui_kit::focus_first(&el);
                        let inside = document()
                            .active_element()
                            .is_some_and(|a| el.contains(Some(a.unchecked_ref())));
                        if !inside {
                            let _ = el.focus();
                        }
                    }
                },
                std::time::Duration::ZERO,
            );
        }
        now
    });
    let key = window_event_listener(leptos::ev::keydown, move |e| {
        if e.key() == "Escape" && open.get_untracked() {
            open.set(false);
            if let Some(t) = html_by_id(trigger) {
                let _ = t.focus();
            }
        }
    });
    let down = window_event_listener(leptos::ev::pointerdown, move |e| {
        if !open.get_untracked() {
            return;
        }
        let target = e.target().and_then(|t| t.dyn_into::<web_sys::Node>().ok());
        let inside = |id: &str| {
            document()
                .get_element_by_id(id)
                .is_some_and(|el| el.contains(target.as_ref()))
        };
        if !inside(panel) && !inside(trigger) {
            open.set(false);
        }
    });
    on_cleanup(move || {
        key.remove();
        down.remove();
    });
}

pub fn read_only_reason(s: Store) -> (String, String) {
    let level = s
        .welcome
        .with(|w| w.as_ref().map(|w| (w.you.level, w.you.visitor.clone())));
    if let Some((AdminLevel::Read, who)) = level {
        return (
            format!("{who} has read admin: you can see everything, a write admin makes changes."),
            String::new(),
        );
    }
    s.state
        .with(|st| {
            st.as_ref()
                .and_then(|st| st.file.why_not.as_ref())
                .map(|w| (w.message.clone(), w.hint.clone()))
        })
        .unwrap_or_else(|| {
            (
                "The server cannot change its config file.".into(),
                String::new(),
            )
        })
}

#[component]
fn Banners() -> impl IntoView {
    let s = store();
    let show_diags = RwSignal::new(false);
    let reconnecting = move || match s.conn.get() {
        Conn::Reconnecting { attempt } => {
            let left = ((s.retry_at.get() - s.now.get()) / 1000.0).ceil().max(0.0) as u64;
            let n = s.edit.pending();
            let at = format::clock(s.values_at.get());
            Some(view! {
                <Banner
                    tone=Tone::Warning
                    title=format!("Lost the connection to {}.", host_line(s))
                    message=format!("Reconnecting in {left} s · attempt {attempt}. Your {n} pending changes are kept. Values are from {at}.")
                    testid="banner-reconnecting"
                >
                    <Button label="Retry now" small=true on_click=move |_| api::reconnect(s)/>
                </Banner>
            })
        }
        _ => None,
    };
    let closed = move || match s.conn.get() {
        Conn::Closed { message } => Some(view! {
            <Banner tone=Tone::Danger title="Disconnected; showing the last known state" message=message testid="banner-closed">
                <Button label="Reconnect" small=true on_click=move |_| api::reconnect(s)/>
            </Banner>
        }),
        _ => None,
    };
    let broken = move || {
        s.state.get().filter(|st| !st.running.ok).map(|st| {
            let lines: Vec<String> = st.running.file_diags.iter().map(|d| d.message.clone()).collect();
            view! {
                <Banner tone=Tone::Danger title="The config file on the server has errors; the server keeps running the previous version" message=lines.first().cloned().unwrap_or_default() testid="banner-file-errors">
                    <Button label=Signal::derive(move || if show_diags.get() { "Hide diagnostics".to_string() } else { "Show diagnostics".to_string() }) small=true on_click=move |_| show_diags.update(|v| *v = !*v)/>
                </Banner>
                <Show when=move || show_diags.get()>
                    <ul class="border-b border-danger/30 bg-danger-weak px-5 py-2 font-mono text-xs text-co-danger">
                        {lines.iter().map(|l| view! { <li>{l.clone()}</li> }).collect_view()}
                    </ul>
                </Show>
            }
        })
    };
    let changed = move || {
        s.changed.get().map(|c| {
            let by = if c.by.is_empty() || c.by == "file" { "by a hand edit".to_string() } else { format!("by visitor {}", c.by) };
            view! {
                <Banner
                    tone=Tone::Warning
                    title=format!("Config changed {by}")
                    message={
                        let (old, at) = s.changed_from.get_untracked();
                        let when = if s.now.get_untracked() - at < 60_000.0 { "noticed just now".to_string() } else { format!("at {}", crate::format::clock(at)) };
                        if old.is_empty() {
                            format!("now rev {} · {when}", short_rev(&c.rev))
                        } else {
                            format!("rev {} → {} · {when}", short_rev(&old), short_rev(&c.rev))
                        }
                    }
                    testid="banner-changed"
                >
                    <Button label="Refresh" small=true on_click=move |_| {
                        api::load_state(s);
                        s.changed.set(None);
                    }/>
                </Banner>
            }
        })
    };
    view! {
        <div data-testid="banners">
            {broken}
            {changed}
            {reconnecting}
            {closed}
        </div>
    }
}

#[derive(Clone, PartialEq)]
struct Hit {
    section: &'static str,
    icon: &'static str,
    label: String,
    facet: String,
    go: Route,
    query: Option<(&'static str, &'static str)>,
}

fn marked(label: &str, needle: &str) -> AnyView {
    let lower = label.to_lowercase();
    match (!needle.is_empty()).then(|| lower.find(needle)).flatten() {
        Some(at) if label.is_char_boundary(at) && label.is_char_boundary(at + needle.len()) => {
            let (a, rest) = label.split_at(at);
            let (b, c) = rest.split_at(needle.len());
            view! { <span>{a.to_string()}<span class="font-bold text-accent-ink">{b.to_string()}</span>{c.to_string()}</span> }.into_any()
        }
        _ => view! { <span>{label.to_string()}</span> }.into_any(),
    }
}

fn search_hits(s: Store, needle: &str) -> Vec<Hit> {
    let doc = s.edit.doc();
    let status = s.status.get_untracked();
    let hit = |l: &str| needle.is_empty() || l.to_lowercase().contains(needle);
    let mut out = Vec::new();
    let names = |sec: &str| {
        doc.get(sec)
            .and_then(Value::as_object)
            .map(|m| m.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default()
    };
    if s.can_write_now() {
        for c in names(ClientEntry::section()) {
            let label = format!("Rotate secret of {c}…");
            if !needle.is_empty() && hit(&label) {
                out.push(Hit {
                    section: "Actions",
                    icon: "refresh",
                    label,
                    facet: ClientEntry::ENTITY.into(),
                    go: Route::Detail(ClientEntry::section().into(), c),
                    query: Some(("tab", "secrets")),
                });
            }
        }
        for (sec, what) in [
            (ServiceEntry::section(), ServiceEntry::ENTITY),
            (ClientEntry::section(), ClientEntry::ENTITY),
            (VisitorEntry::section(), VisitorEntry::ENTITY),
        ] {
            let label = format!("New {what}…");
            if hit(&label) {
                out.push(Hit {
                    section: "Actions",
                    icon: "plus",
                    label,
                    facet: String::new(),
                    go: Route::List(sec.into()),
                    query: Some(("new", "1")),
                });
            }
        }
        let n = s.edit.pending();
        if n > 0 {
            let label = format!("Review {n} change{}", if n == 1 { "" } else { "s" });
            if hit(&label) {
                out.push(Hit {
                    section: "Actions",
                    icon: "diff",
                    label,
                    facet: String::new(),
                    go: Route::Review,
                    query: None,
                });
            }
        }
    }
    for (sec, title, icon) in [
        (ServiceEntry::section(), "Services", "stack"),
        (ClientEntry::section(), "Clients", "monitor"),
        (VisitorEntry::section(), "Visitors", "person"),
    ] {
        for n in names(sec) {
            if !hit(&n) {
                continue;
            }
            let v = doc
                .get(sec)
                .and_then(|m| m.get(&n))
                .cloned()
                .unwrap_or(Value::Null);
            let facet = match entity_of_section(sec) {
                Some(Entity::Client) => {
                    let c = status
                        .as_ref()
                        .and_then(|st| st.clients.iter().find(|c| c.name == n))
                        .cloned();
                    let secrets = match ClientEntry::SECRET.get(&v) {
                        Some(Value::Array(a)) => a.len(),
                        Some(_) => 1,
                        None => 0,
                    };
                    match c {
                        Some(c) if c.connected => format!(
                            "{} · {secrets} secret{}",
                            c.from
                                .unwrap_or_default()
                                .rsplit_once(':')
                                .map(|(h, _)| h.to_string())
                                .unwrap_or_default(),
                            if secrets == 1 { "" } else { "s" }
                        ),
                        _ => format!(
                            "offline · {secrets} secret{}",
                            if secrets == 1 { "" } else { "s" }
                        ),
                    }
                }
                Some(Entity::Service) => ServiceEntry::BIND
                    .get(&v)
                    .filter(|b| !b.is_null())
                    .map(crate::form::plain)
                    .unwrap_or_else(|| "private".into()),
                _ => VisitorEntry::ADMIN
                    .get(&v)
                    .map(|a| format!("admin · {}", crate::form::plain(a)))
                    .unwrap_or_else(|| "visitor".into()),
            };
            out.push(Hit {
                section: title,
                icon,
                label: n.clone(),
                facet,
                go: Route::Detail(sec.into(), n),
                query: None,
            });
        }
    }
    if let Some(schema) = s.schema.get_untracked() {
        let skip: Vec<String> = schema
            .entities
            .iter()
            .filter_map(|e| e.path.first().cloned())
            .collect();
        for f in burrow_admin_logic::tree::fields(&schema.root)
            .iter()
            .filter(|f| !skip.contains(&f.key))
        {
            if !needle.is_empty() && (hit(&f.node.label) || hit(&f.key)) {
                out.push(Hit {
                    section: "Settings",
                    icon: "settings",
                    label: f.node.label.clone(),
                    facet: f.key.clone(),
                    go: Route::Settings,
                    query: None,
                });
            }
        }
    }
    if !needle.is_empty() {
        for e in s.history.get_untracked().unwrap_or_default() {
            let summary = e
                .changes
                .first()
                .map(|c| c.summary.clone())
                .unwrap_or_default();
            if hit(&summary) {
                out.push(Hit {
                    section: "Activity",
                    icon: "history",
                    label: format!("rev {} · {summary}", short_rev(&e.rev)),
                    facet: crate::format::relative(e.at),
                    go: Route::Activity(Some(e.rev.clone())),
                    query: None,
                });
            }
        }
    }
    for (r, l, i) in [
        (Route::Settings, "Settings", "settings"),
        (Route::Activity(None), "Activity", "activity"),
        (Route::About, "About", "about"),
    ] {
        if needle.is_empty() || l.to_lowercase().contains(needle) {
            out.push(Hit {
                section: "Pages",
                icon: i,
                label: l.to_string(),
                facet: String::new(),
                go: r,
                query: None,
            });
        }
    }
    out.truncate(14);
    out
}

#[component]
fn SearchPopover() -> impl IntoView {
    let s = store();
    let q = RwSignal::new(String::new());
    let active = RwSignal::new(0usize);
    let input: NodeRef<leptos::html::Input> = NodeRef::new();
    Effect::new(move |_| {
        if s.search_open.get() {
            q.set(String::new());
            active.set(0);
            if let Some(i) = input.get() {
                let _ = i.focus();
            }
        }
    });
    let hits = Memo::new(move |_| {
        if !s.search_open.get() {
            return Vec::new();
        }
        search_hits(s, &q.get().trim().to_lowercase())
    });
    let run = move |h: Hit| {
        s.search_open.set(false);
        route::go(h.go.clone());
        if let Some((k, v)) = h.query {
            route::set_query(k, v);
            if k == "new" {
                s.creating.set(Some(h.go.section().to_string()));
            }
        }
    };
    let body = move || {
        let list = hits.get();
        let needle = q.get().trim().to_lowercase();
        let act = active.get().min(list.len().saturating_sub(1));
        let mut last = "";
        list.into_iter()
            .enumerate()
            .map(|(i, h)| {
                let head = (h.section != last).then(|| view! { <li class="px-2.5 pt-2.5 pb-1 text-2xs font-bold tracking-[0.08em] text-text-faint uppercase" role="presentation">{h.section}</li> });
                last = h.section;
                let h2 = h.clone();
                view! {
                    {head}
                    <li role="option" aria-selected=(i == act).to_string()>
                        <button
                            type="button"
                            class="flex w-full cursor-pointer items-center gap-2.5 rounded-md px-2.5 py-2 text-left text-sm"
                            class:bg-surface-3=i == act
                            on:mouseenter=move |_| active.set(i)
                            on:click=move |_| run(h2.clone())
                        >
                            <span class="text-text-muted"><Icon name=h.icon/></span>
                            {marked(&h.label, &needle)}
                            <span class="ml-auto truncate text-xs text-text-muted">{h.facet.clone()}</span>
                            {(i == act).then(|| view! { <Kbd text="↵"/> })}
                        </button>
                    </li>
                }
            })
            .collect_view()
    };
    view! {
        <Show when=move || s.search_open.get()>
            <div class="fixed inset-0 z-40 bg-scrim" on:click=move |_| s.search_open.set(false)></div>
            <div
                class="fixed top-[84px] left-1/2 z-50 w-[min(640px,calc(100vw-32px))] -translate-x-1/2 overflow-hidden rounded-dialog border border-border-strong bg-surface shadow-overlay dark:bg-surface-2"
                role="dialog"
                aria-modal="true"
                aria-label="Search"
                data-testid="search"
                on:keydown=move |e| match e.key().as_str() {
                    "Escape" => s.search_open.set(false),
                    "ArrowDown" => {
                        e.prevent_default();
                        let n = hits.with(Vec::len);
                        if n > 0 {
                            active.update(|a| *a = (*a + 1).min(n - 1));
                        }
                    }
                    "ArrowUp" => {
                        e.prevent_default();
                        active.update(|a| *a = a.saturating_sub(1));
                    }
                    "Enter" => {
                        e.prevent_default();
                        if let Some(h) = hits.with(|l| l.get(active.get_untracked()).cloned()) {
                            run(h);
                        }
                    }
                    _ => {}
                }
            >
                <div class="flex items-center gap-2.5 border-b border-border px-4">
                    <Icon name="search"/>
                    <input
                        node_ref=input
                        type="text"
                        role="combobox"
                        aria-expanded="true"
                        aria-label="Search services, clients, visitors, settings and history"
                        placeholder="Search or run an action"
                        class="h-[50px] flex-1 bg-transparent text-base outline-none placeholder:text-text-faint"
                        prop:value=move || q.get()
                        on:input:target=move |e| {
                            q.set(e.target().value());
                            active.set(0);
                        }
                    />
                    <Kbd text="esc"/>
                </div>
                <ul class="max-h-[60vh] overflow-y-auto p-1.5" role="listbox">{body}</ul>
                <div class="flex items-center gap-3 border-t border-border px-4 py-2.5 text-xs text-text-muted">
                    <span>"↑↓ move"</span>
                    <span>"↵ run"</span>
                    <span class="ml-auto">"Searches services, clients, visitors, settings and history"</span>
                </div>
            </div>
        </Show>
    }
}
