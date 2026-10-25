mod header;
mod menus;
mod search;

use burrow_admin_proto::msg::{FileDiagnostics, UiDiagnostics};
use burrow_admin_proto::serde_json;
use burrow_ui_kit::{Banner, Button, EmptyState, Skeleton, ToastHost, Tone};
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;

use crate::api;
use crate::format;
use crate::review::ConflictDialog;
use crate::route::{self, Route};
use crate::screens;
use crate::store::{Conn, Store, short_rev, store};

use header::Header;
pub(crate) use header::popover;
use search::SearchPopover;

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
    let on_pop = Closure::<dyn FnMut()>::new(move || {
        s.route.set(route::current());
        s.query_seen.update(|n| *n += 1);
    });
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
    for ev in ["focusin", "click", "input", "keyup"] {
        let cb = wasm_bindgen::closure::Closure::<dyn Fn()>::new(move || {
            request_animation_frame(screens::clear_of_bar);
        });
        let _ = document().add_event_listener_with_callback(ev, cb.as_ref().unchecked_ref());
        cb.forget();
    }
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
            toasts=Signal::derive(move || {
                let on_review = s.route.with(|r| *r == Route::Review);
                s.toasts
                    .get()
                    .into_iter()
                    .map(|mut t| {
                        if on_review {
                            t.action = None;
                        }
                        t
                    })
                    .collect::<Vec<_>>()
            })
            on_action=move |_| route::go(Route::Review)
            on_hover=move |h: bool| s.toast_hold.set(h)
        />
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
            Conn::Reconnecting { .. } | Conn::Closed { .. } | Conn::Ended
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
            Conn::Ended if !loaded.get() => view! {
                <div class="mx-auto max-w-2xl p-8" data-testid="ended">
                    <EmptyState icon="wifi-off" title="This tab's admin session ended" message="Run burrow admin again; it opens a new tab."/>
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
            let (title, why) = if s.admin_down.get() && api::gone_a_while(attempt) {
                (
                    "burrow admin is not running here.".to_string(),
                    "If burrow admin started again, open the link it printed or run burrow admin. "
                        .to_string(),
                )
            } else if s.admin_down.get() {
                (
                    "burrow admin is not running.".to_string(),
                    "Start it again and this tab reconnects. ".to_string(),
                )
            } else {
                (
                    format!("Lost the connection to {}.", s.server_name()),
                    String::new(),
                )
            };
            let kept = match n {
                0 => String::new(),
                1 => " Your 1 pending change is kept.".to_string(),
                n => format!(" Your {n} pending changes are kept."),
            };
            Some(view! {
                <Banner
                    tone=Tone::Warning
                    title=title
                    message=format!("{why}Reconnecting in {left} s · attempt {attempt}.{kept} Values are from {at}.")
                    testid="banner-reconnecting"
                >
                    <Button label="Retry now" small=true on_click=move |_| api::reconnect(s)/>
                </Banner>
            })
        }
        _ => None,
    };
    let closed = move || {
        match s.conn.get() {
        Conn::Ended if s.state.with(Option::is_some) => Some(view! {
            <Banner tone=Tone::Warning title="This tab's admin session ended." message="Run burrow admin again; it opens a new tab." testid="banner-ended"/>
        }.into_any()),
        Conn::Closed { message } => Some(view! {
            <Banner tone=Tone::Danger title="Disconnected; showing the last known state" message=message testid="banner-closed">
                <Button label="Reconnect" small=true on_click=move |_| api::reconnect(s)/>
            </Banner>
        }.into_any()),
        _ => None,
    }
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
        let stale = crate::review::stale(s);
        let c = s.changed.get();
        if c.is_none() && stale.is_none() {
            return None;
        }
        let by = match &c {
            Some(c) if !c.by.is_empty() && c.by != "file" => format!("by visitor {}", c.by),
            Some(_) => "by a hand edit".to_string(),
            None => "on the server".to_string(),
        };
        let revs = match (&stale, &c) {
            (Some((base, now)), _) => (base.clone(), now.clone()),
            (None, Some(c)) => (s.changed_from.get_untracked().0, c.rev.clone()),
            (None, None) => (String::new(), String::new()),
        };
        let action = match stale {
            Some((_, now)) => view! {
                <Button label=format!("Review my draft on rev {}", short_rev(&now)) small=true testid="banner-recheck" on_click=move |_| crate::review::recheck(s)/>
            }
            .into_any(),
            None => view! {
                <Button label="Refresh" small=true on_click=move |_| {
                    api::load_state(s);
                    s.changed.set(None);
                }/>
            }
            .into_any(),
        };
        Some(view! {
            <Banner
                tone=Tone::Warning
                title=format!("Config changed {by}")
                message=move || {
                    let (_, at) = s.changed_from.get();
                    let when = if at == 0.0 || s.now.get() - at < 60_000.0 { "noticed just now".to_string() } else { format!("noticed at {}", crate::format::clock(at)) };
                    if revs.0.is_empty() {
                        format!("now rev {} · {when}", short_rev(&revs.1))
                    } else {
                        format!("rev {} → {} · {when}", short_rev(&revs.0), short_rev(&revs.1))
                    }
                }
                testid="banner-changed"
            >
                {action}
            </Banner>
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
