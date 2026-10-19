use burrow_admin_proto::msg::Level;
use burrow_ui_kit::{Banner, Button, EmptyState, Icon, Skeleton, Tone, button_class};
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;

use crate::api;
use crate::format;
use crate::route::{self, Link, Route};
use crate::screens;
use crate::store::{Conn, Store, store};

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
    api::connect(s);
    view! {
        <a class="sr-only focus:not-sr-only focus:absolute focus:top-2 focus:left-2 focus:z-50 focus:bg-surface focus:p-2" href="#main">"Skip to content"</a>
        <div class="grid min-h-screen grid-cols-[3.5rem_1fr] grid-rows-[3rem_1fr] md:grid-cols-[13.75rem_1fr]">
            <TopBar/>
            <SideNav/>
            <main id="main" class="min-w-0 p-4 md:p-6" tabindex="-1">
                <Body/>
            </main>
        </div>
    }
}

#[component]
fn TopBar() -> impl IntoView {
    let s = store();
    let pill = move || {
        let conn = s.conn.get();
        let (tone, text) = match &conn {
            Conn::Open if !s.can_write() => (Tone::Warning, "Read-only".to_string()),
            Conn::Open => (Tone::Success, "Connected".to_string()),
            Conn::Reconnecting { attempt } => {
                (Tone::Warning, format!("Reconnecting (try {attempt})"))
            }
            Conn::Connecting => (Tone::Info, "Connecting".to_string()),
            other => (Tone::Danger, other.label().to_string()),
        };
        view! {
            <span
                class=format!("inline-flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-sm whitespace-nowrap {}", tone.soft())
                data-testid="conn-pill"
                data-tone=tone.name()
                role="status"
            >
                <span class="size-2 rounded-full bg-current" aria-hidden="true"></span>
                {text}
            </span>
        }
    };
    let server = move || {
        s.welcome.get().map(|w| {
            let name = if w.server.name_hint.is_empty() {
                "burrow server".to_string()
            } else {
                w.server.name_hint.clone()
            };
            let up = s
                .status
                .get()
                .map(|st| st.uptime_secs)
                .unwrap_or(w.server.uptime_secs);
            format!("{name} · v{} · up {}", w.server.version, format::span(up))
        })
    };
    let you = move || {
        s.welcome.get().map(|w| {
            let level = match w.you.level {
                Level::Write => "write",
                Level::Read => "read",
            };
            format!("{} ({level})", w.you.visitor)
        })
    };
    let theme = RwSignal::new(saved_theme());
    let toggle = move |_| {
        let next = match theme.get_untracked().as_str() {
            "dark" => "light",
            "light" => "",
            _ => "dark",
        };
        theme.set(next.to_string());
        apply_theme(next);
        if let Some(st) = window().local_storage().ok().flatten() {
            let _ = st.set_item(THEME_KEY, next);
        }
    };
    view! {
        <header class="sticky top-0 z-30 col-span-full flex h-12 items-center justify-between gap-4 border-b border-border bg-surface px-4">
            <div class="flex min-w-0 items-center gap-2">
                <span class="text-accent" aria-hidden="true">"◆"</span>
                <span class="font-semibold whitespace-nowrap">"burrow admin"</span>
                <span class="hidden truncate text-sm text-text-muted lg:inline" data-testid="server-line">{server}</span>
            </div>
            <div class="flex items-center gap-3">
                {pill}
                <span class="hidden font-mono text-sm text-text-muted md:inline" data-testid="you">{you}</span>
                <button
                    type="button"
                    class=button_class("ghost", true)
                    data-testid="theme-toggle"
                    on:click=toggle
                >
                    {move || match theme.get().as_str() {
                        "dark" => "Theme: dark",
                        "light" => "Theme: light",
                        _ => "Theme: system",
                    }}
                </button>
            </div>
        </header>
    }
}

fn count(s: Store, section: &'static str) -> Option<String> {
    let total = s.state.with(|st| {
        st.as_ref().and_then(|st| {
            st.values
                .get(section)
                .and_then(|v| v.as_object())
                .map(|m| m.len())
        })
    })?;
    if section == "clients" {
        let online = s.status.with(|st| {
            st.as_ref()
                .map(|st| st.clients.iter().filter(|c| c.connected).count())
                .unwrap_or(0)
        });
        return Some(format!("{online}/{total}"));
    }
    Some(total.to_string())
}

#[component]
fn SideNav() -> impl IntoView {
    let s = store();
    let item = move |to: Route,
                     label: &'static str,
                     icon: &'static str,
                     counted: Option<&'static str>| {
        let key = to.section().to_string();
        let current = {
            let key = key.clone();
            move || s.route.with(|r| r.section() == key)
        };
        view! {
            <li>
                <Link to=to class="block no-underline" testid="nav">
                    <span
                        class="flex items-center gap-2 rounded-input px-2 py-2 text-text hover:bg-surface-2 aria-[current=page]:bg-accent-weak aria-[current=page]:font-semibold aria-[current=page]:text-accent"
                        aria-current=move || current().then_some("page")
                        data-nav=key.clone()
                    >
                        <Icon name=icon/>
                        <span class="flex-1 max-md:sr-only">{label}</span>
                        <span class="text-xs text-text-muted tabular-nums max-md:hidden">{move || counted.and_then(|c| count(s, c))}</span>
                    </span>
                </Link>
            </li>
        }
    };
    view! {
        <nav class="border-r border-border bg-surface p-2 md:p-3" aria-label="Sections">
            <ul>
                {item(Route::Overview, "Overview", "overview", None)}
                {item(Route::List("services".into()), "Services", "services", Some("services"))}
                {item(Route::List("clients".into()), "Clients", "clients", Some("clients"))}
                {item(Route::List("visitors".into()), "Visitors", "visitors", Some("visitors"))}
                {item(Route::Settings, "Settings", "settings", None)}
            </ul>
            <ul class="mt-4 border-t border-border pt-4">
                {item(Route::Activity, "Activity", "activity", None)}
                {item(Route::About, "About", "about", None)}
            </ul>
        </nav>
    }
}

#[component]
fn Body() -> impl IntoView {
    let s = store();
    move || {
        let conn = s.conn.get();
        let loaded = s.state.with(Option::is_some) && s.schema.with(Option::is_some);
        match conn {
            Conn::NoToken => view! {
                <EmptyState
                    title="Open burrow admin from the terminal"
                    message="This page needs the one-time link that burrow admin opens. Run burrow admin again on this machine."
                    testid="no-token"
                />
            }
            .into_any(),
            Conn::Refused { reason, message } => view! {
                <div data-testid="refused" data-reason=reason.clone()>
                    <Banner tone=Tone::Danger title="The server refused the admin session" message=message/>
                </div>
            }
            .into_any(),
            Conn::Revoked { reason } => view! {
                <div data-testid="revoked">
                    <Banner
                        tone=Tone::Danger
                        title="Your admin access was removed"
                        message=format!("The server closed this session: {reason}")
                    />
                </div>
            }
            .into_any(),
            Conn::Closed { message } if !loaded => view! {
                <div data-testid="closed">
                    <Banner tone=Tone::Danger title="Disconnected" message=message>
                        <Button label="Reconnect" on_click=move |_| api::reconnect(s)/>
                    </Banner>
                </div>
            }
            .into_any(),
            _ if !loaded => view! {
                <div data-testid="loading">
                    <Skeleton lines=6/>
                </div>
            }
            .into_any(),
            _ => view! {
                <Banners/>
                {move || screens::screen(s.route.get())}
            }
            .into_any(),
        }
    }
}

#[component]
fn Banners() -> impl IntoView {
    let s = store();
    let closed = move || match s.conn.get() {
        Conn::Closed { message } => Some(view! {
            <div data-testid="banner-closed">
                <Banner tone=Tone::Danger title="Disconnected; showing the last known state" message=message>
                    <Button label="Reconnect" on_click=move |_| api::reconnect(s)/>
                </Banner>
            </div>
        }),
        _ => None,
    };
    let broken = move || {
        s.state.get().filter(|st| !st.running.ok).map(|st| {
            let first = st
                .running
                .file_diags
                .first()
                .map(|d| d.message.clone())
                .unwrap_or_default();
            view! {
                <Banner
                    tone=Tone::Danger
                    title="The config file on the server has errors; the server keeps running the previous version"
                    message=first
                    testid="banner-file-errors"
                />
            }
        })
    };
    let changed = move || {
        s.changed.get().map(|c| {
            let by = if c.by.is_empty() || c.by == "file" {
                "by a hand edit".to_string()
            } else {
                format!("by visitor {}", c.by)
            };
            view! {
                <Banner
                    tone=Tone::Info
                    title=format!("The config changed {by}")
                    message="This page now shows the new version."
                    testid="banner-changed"
                >
                    <Button label="Dismiss" small=true on_click=move |_| s.changed.set(None)/>
                </Banner>
            }
        })
    };
    let read_only = move || {
        let w = s.welcome.get()?;
        let st = s.state.get()?;
        if w.you.level == Level::Read {
            return Some(view! {
                <Banner
                    tone=Tone::Warning
                    title="Read-only: your visitor has read access"
                    message=format!("To change settings, set admin: write for visitors.{} in server.yaml on the server.", w.you.visitor)
                    testid="banner-read-only"
                />
            });
        }
        if !st.file.writable {
            let (msg, hint) = st
                .file
                .why_not
                .as_ref()
                .map(|w| (w.message.clone(), w.hint.clone()))
                .unwrap_or_default();
            return Some(view! {
                <Banner
                    tone=Tone::Warning
                    title=format!("Read-only: {msg}")
                    message=hint
                    testid="banner-read-only"
                />
            });
        }
        None
    };
    let restart = move || {
        s.state
            .get()
            .filter(|st| !st.runtime.restart_pending.is_empty())
            .map(|st| {
                view! {
                    <Banner
                        tone=Tone::Warning
                        title="Restart needed"
                        message=format!("The server applies these after a restart: {}", st.runtime.restart_pending.join(", "))
                        testid="banner-restart"
                    />
                }
            })
    };
    view! {
        <div class="mb-2">
            {closed}
            {broken}
            {changed}
            {read_only}
            {restart}
        </div>
    }
}
