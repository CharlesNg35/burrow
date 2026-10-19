use burrow_admin_proto::msg::Level;
use burrow_admin_proto::serde_json::{Value, json};
use burrow_ui_kit::{Banner, Button, EmptyState, Icon, Skeleton, ToastHost, Tone, copy_text};
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
    json!({
        "ui": env!("CARGO_PKG_VERSION"),
        "server": w.as_ref().map(|w| &w.server.version),
        "target": w.as_ref().map(|w| &w.server.target),
        "protocol": w.as_ref().map(|w| w.protocol),
        "features": w.as_ref().map(|w| &w.features),
        "schema_hash": schema.as_ref().map(|s| &s.hash),
        "schema_version": schema.as_ref().map(|s| s.schema_version),
        "file": st.as_ref().map(|s| json!({"format": s.file.format, "writable": s.file.writable, "running_ok": s.running.ok})),
    })
    .to_string()
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
                    <crate::secrets::ShownOnce/>
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
            st.values
                .get("public_address")
                .and_then(Value::as_str)
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
        (Route::List("services".into()), "Services", "services"),
        (Route::List("clients".into()), "Clients", "clients"),
        (Route::List("visitors".into()), "Visitors", "visitors"),
        (Route::Settings, "Settings", "settings"),
        (Route::Activity(None), "Activity", "activity"),
    ];
    let nav = tabs
        .into_iter()
        .map(|(to, label, icon)| {
            let key = to.section().to_string();
            let current = move || s.route.with(|r| r.section() == key);
            view! {
                <Link to=to class="flex h-[34px] items-center gap-1.5 rounded-md px-3 text-[13px] font-semibold text-hdr-muted no-underline hover:text-hdr-on aria-[current=page]:bg-tab-on aria-[current=page]:text-hdr-on aria-[current=page]:shadow-sm aria-[current=page]:ring-1 aria-[current=page]:ring-border aria-[current=page]:ring-inset max-xl:px-2.5 dark:aria-[current=page]:ring-border-strong" testid="nav" current=Signal::derive(current.clone())>
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
    let conn_state = move || {
        let conn = s.conn.get();
        match conn {
            Conn::Reconnecting { .. } => {
                let left = ((s.retry_at.get() - s.now.get()) / 1000.0).ceil().max(0.0) as u64;
                view! { <span class="flex items-center gap-1.5 text-sm text-warning" data-testid="conn-pill"><Icon name="refresh"/>{format!("Reconnecting in {left} s")}</span> }.into_any()
            }
            Conn::Open => view! { <span class="flex items-center gap-1.5 text-sm text-hdr-text max-sm:sr-only" data-testid="conn-pill"><span class="size-[7px] rounded-full bg-success"></span>{if read_only.get() { "Read-only" } else { "Connected" }}</span> }.into_any(),
            Conn::Connecting => view! { <span class="text-sm text-hdr-muted" data-testid="conn-pill">"Connecting"</span> }.into_any(),
            other => view! { <span class="flex items-center gap-1.5 text-sm text-danger" data-testid="conn-pill"><Icon name="wifi-off"/>{other.label()}</span> }.into_any(),
        }
    };
    let dot = move || match s.conn.get() {
        Conn::Open if read_only.get() => "bg-text-faint",
        Conn::Open => "bg-success",
        Conn::Reconnecting { .. } | Conn::Connecting => "bg-warning",
        _ => "bg-danger",
    };
    let you = Memo::new(move |_| {
        s.welcome.with(|w| {
            w.as_ref().map(|w| {
                (
                    w.you.visitor.clone(),
                    match w.you.level {
                        Level::Write => "write",
                        Level::Read => "read",
                    },
                )
            })
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
        <header class="sticky top-0 z-30 grid grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] items-center gap-4 border-b border-hdr-line bg-hdr px-5 text-hdr-text max-xl:grid-cols-[minmax(0,1fr)_auto] max-xl:pt-3 xl:h-16" data-testid="header">
            <div class="relative flex min-w-0 items-center gap-2.5">
                <span class="flex size-[30px] shrink-0 items-center justify-center rounded-md bg-gradient-to-b from-logo-from to-logo-to text-white" aria-hidden="true"><Icon name="logo" large=true/></span>
                <button type="button" class="flex min-w-0 cursor-pointer items-center gap-2 rounded-md px-1.5 py-1 hover:bg-hdr-raised" aria-haspopup="true" aria-expanded=move || menu_open.get().to_string() data-testid="server-switcher" on:click=move |_| menu_open.update(|o| *o = !*o)>
                    <span class="truncate font-semibold text-hdr-on" data-testid="server-line">{move || s.server_name()}</span>
                    {move || {
                        let host = host_line(s);
                        (host != s.server_name()).then(|| view! { <span class="truncate font-mono text-xs text-hdr-muted max-sm:hidden">{host}</span> })
                    }}
                    <Icon name="chevron-down" small=true/>
                </button>
                <Show when=move || menu_open.get()>
                    <div class="absolute top-11 left-10 z-40 w-80 rounded-[10px] border border-border-strong bg-surface p-1 text-text shadow-overlay dark:bg-surface-2" data-testid="server-menu" on:click=move |_| menu_open.set(false)>
                        {facts}
                        <div class="border-t border-border p-1">
                            <Link to=Route::About class="flex items-center gap-2 rounded-md px-2 py-1.5 text-sm no-underline hover:bg-surface-3">"About this server"</Link>
                        </div>
                    </div>
                </Show>
            </div>
            <nav aria-label="Sections" class="flex gap-0.5 rounded-[11px] border border-hdr-line bg-hdr-raised p-[3px] max-xl:col-span-2 max-xl:row-start-2 max-xl:mb-3 max-xl:justify-self-center max-sm:max-w-full max-sm:overflow-x-auto dark:border-hdr-raised-2">
                {nav}
            </nav>
            <div class="flex items-center justify-end gap-3">
                <button type="button" class="flex size-8 cursor-pointer items-center justify-center rounded-md border border-hdr-line bg-hdr-raised text-hdr-text dark:border-hdr-raised-2" aria-label="Search (/)" data-testid="search-open" on:click=move |_| s.search_open.set(true)>
                    <Icon name="search"/>
                </button>
                {conn_state}
                {move || {
                    if read_only.get() {
                        view! { <span class="inline-flex h-6 items-center gap-1 rounded-[5px] border border-border bg-surface-3 px-2 text-xs font-semibold text-text-muted" data-testid="read-only-tag"><Icon name="eye" small=true/>"read-only"</span> }.into_any()
                    } else if pending.get() > 0 {
                        view! {
                            <Link to=Route::Review class="inline-flex h-8 items-center gap-1.5 rounded-md bg-accent px-3 text-sm font-semibold text-on-accent no-underline hover:brightness-110" testid="review-open" state=check_state>
                                <Icon name="diff"/>
                                "Review"
                                <span class="rounded-sm bg-white/25 px-1.5 text-[11.5px] dark:bg-black/18" data-testid="review-count">{pending.get()}</span>
                            </Link>
                        }
                        .into_any()
                    } else {
                        ().into_any()
                    }
                }}
                <div class="relative flex items-center gap-2">
                    <button type="button" class="relative flex size-[30px] cursor-pointer items-center justify-center rounded-full bg-hdr-raised-2 text-xs font-bold text-hdr-on" aria-label=move || format!("Account {}", you.get().map(|y| y.0).unwrap_or_default()) aria-haspopup="true" aria-expanded=move || avatar_open.get().to_string() data-testid="avatar" on:click=move |_| avatar_open.update(|o| *o = !*o)>
                        {move || you.get().map(|y| initials(&y.0)).unwrap_or_default()}
                        <span class=move || format!("absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full ring-2 ring-hdr {}", dot())></span>
                    </button>
                    <span class="hidden flex-col leading-tight 2xl:flex" data-testid="you">
                        <span class="text-sm font-semibold text-hdr-on">{move || you.get().map(|y| y.0).unwrap_or_default()}</span>
                        <span class="text-[11px] text-hdr-muted">{move || you.get().map(|y| format!("admin · {}", y.1)).unwrap_or_default()}</span>
                    </span>
                    <span class="sr-only" data-testid="you-level">{move || you.get().map(|y| format!("{} ({})", y.0, y.1)).unwrap_or_default()}</span>
                    <Show when=move || avatar_open.get()>
                        <div class="absolute top-10 right-0 z-40 w-60 rounded-[10px] border border-border-strong bg-surface p-1 text-text shadow-overlay dark:bg-surface-2" data-testid="avatar-menu">
                            <div class="px-2 pt-1 pb-1.5 text-2xs font-bold tracking-[0.08em] text-text-faint uppercase">"Theme"</div>
                            {[("", "System"), ("light", "Light"), ("dark", "Dark")]
                                .into_iter()
                                .map(|(t, l)| view! {
                                    <button type="button" class="flex w-full cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-surface-3" data-theme-choice=if t.is_empty() { "system" } else { t } on:click=move |_| { set_theme(t); avatar_open.set(false); }>
                                        <span class="w-4">{move || (theme.get() == t).then(|| view! { <Icon name="check" small=true/> })}</span>
                                        {l}
                                    </button>
                                })
                                .collect_view()}
                            <div class="mt-1 border-t border-border pt-1">
                                <button type="button" class="flex w-full cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-surface-3" data-testid="copy-diagnostics" on:click=move |_| {
                                    copy_text(&diagnostics(s));
                                    avatar_open.set(false);
                                    s.toast(Tone::Success, "Copied diagnostics", None);
                                }>
                                    <Icon name="copy" small=true/>
                                    "Copy diagnostics"
                                </button>
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
                <Banner tone=Tone::Warning title=format!("Config changed {by}") message=format!("now at {}", short_rev(&c.rev)) testid="banner-changed">
                    <Button label="Refresh" small=true on_click=move |_| {
                        api::load_state(s);
                        s.changed.set(None);
                    }/>
                </Banner>
            }
        })
    };
    let read_only = move || {
        let w = s.welcome.get()?;
        let st = s.state.get()?;
        if w.you.level == Level::Read {
            let who = w.you.visitor.clone();
            let req = format!(
                "Please give visitor {who} write admin: set admin: write for visitors.{who} in server.yaml on the server."
            );
            return Some(view! {
                <Banner tone=Tone::Info title="Read-only." message=format!("{who} has read admin: you can see everything, a write admin makes changes.") testid="banner-read-only">
                    <Button label="Copy request for a write admin" small=true on_click=move |_| {
                        copy_text(&req);
                        s.toast(Tone::Success, "Copied", None);
                    }/>
                </Banner>
            }.into_any());
        }
        if !st.file.writable {
            let (msg, hint) = st
                .file
                .why_not
                .as_ref()
                .map(|w| (w.message.clone(), w.hint.clone()))
                .unwrap_or_default();
            return Some(view! { <Banner tone=Tone::Info title=format!("Read-only: {msg}") message=hint testid="banner-read-only"/> }.into_any());
        }
        None
    };
    let restart = move || {
        s.state.get().filter(|st| !st.runtime.restart_pending.is_empty()).map(|st| {
            view! { <Banner tone=Tone::Info title="Restart needed for:" message=st.runtime.restart_pending.join(", ") testid="banner-restart"/> }
        })
    };
    view! {
        <div data-testid="banners">
            {broken}
            {changed}
            {read_only}
            {restart}
            {reconnecting}
            {closed}
        </div>
    }
}

#[component]
fn SearchPopover() -> impl IntoView {
    let s = store();
    let q = RwSignal::new(String::new());
    let input: NodeRef<leptos::html::Input> = NodeRef::new();
    Effect::new(move |_| {
        if s.search_open.get() {
            q.set(String::new());
            if let Some(i) = input.get() {
                let _ = i.focus();
            }
        }
    });
    let results = move || {
        let needle = q.get().to_lowercase();
        let doc = s.edit.doc();
        let mut out: Vec<(Route, String, &'static str)> = Vec::new();
        for (sec, icon) in [
            ("services", "services"),
            ("clients", "clients"),
            ("visitors", "visitors"),
        ] {
            for name in doc
                .get(sec)
                .and_then(Value::as_object)
                .map(|m| m.keys().cloned().collect::<Vec<_>>())
                .unwrap_or_default()
            {
                if needle.is_empty() || name.to_lowercase().contains(&needle) {
                    out.push((Route::Detail(sec.into(), name.clone()), name, icon));
                }
            }
        }
        for (r, l, i) in [
            (Route::Settings, "Settings", "settings"),
            (Route::Activity(None), "Activity", "activity"),
            (Route::About, "About", "about"),
        ] {
            if needle.is_empty() || l.to_lowercase().contains(&needle) {
                out.push((r, l.to_string(), i));
            }
        }
        out.truncate(12);
        out
    };
    view! {
        <Show when=move || s.search_open.get()>
            <div class="fixed inset-0 z-40 bg-scrim" on:click=move |_| s.search_open.set(false)></div>
            <div class="fixed top-20 left-1/2 z-50 w-[min(560px,calc(100vw-32px))] -translate-x-1/2 rounded-dialog border border-border-strong bg-surface shadow-overlay dark:bg-surface-2" role="dialog" aria-modal="true" aria-label="Search" data-testid="search" on:keydown=move |e| if e.key() == "Escape" { s.search_open.set(false) }>
                <div class="flex items-center gap-2 border-b border-border px-3">
                    <Icon name="search"/>
                    <input node_ref=input type="text" aria-label="Search services, clients, visitors and pages" placeholder="Search services, clients, visitors" class="h-11 flex-1 bg-transparent text-base outline-none placeholder:text-text-faint" prop:value=move || q.get() on:input:target=move |e| q.set(e.target().value())/>
                </div>
                <ul class="max-h-80 overflow-y-auto p-1">
                    {move || results().into_iter().map(|(r, label, icon)| {
                        let r2 = r.clone();
                        view! {
                            <li>
                                <button type="button" class="flex w-full cursor-pointer items-center gap-2 rounded-md px-2.5 py-2 text-left text-sm hover:bg-surface-3" on:click=move |_| {
                                    s.search_open.set(false);
                                    route::go(r2.clone());
                                }>
                                    <span class="text-text-muted"><Icon name=icon/></span>
                                    <span class="font-semibold">{label}</span>
                                    <span class="ml-auto text-xs text-text-muted">{r.section().to_string()}</span>
                                </button>
                            </li>
                        }
                    }).collect_view()}
                </ul>
            </div>
        </Show>
    }
}
