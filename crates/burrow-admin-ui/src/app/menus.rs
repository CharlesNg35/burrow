use burrow_ui_kit::{Button, Icon, Tone, copy_text};
use leptos::prelude::*;

use crate::api;
use crate::route::{Link, Route};
use crate::store::{Conn, Store};

use super::diagnostics;
use super::header::{read_only_reason, set_theme};

fn retry_left(s: Store) -> u64 {
    ((s.retry_at.get() - s.now.get()) / 1000.0).ceil().max(0.0) as u64
}

pub(super) fn conn_state(s: Store, retry_open: RwSignal<bool>) -> impl Fn() -> AnyView + use<> {
    let shape = Memo::new(move |_| s.conn.with(Conn::shape));
    move || {
        let conn = shape.get();
        match conn {
            Conn::Reconnecting { .. } => {
                view! {
                    <span class="relative">
                        <button type="button" class="flex cursor-pointer items-center gap-1.5 text-[12.5px] text-warning" aria-haspopup="true" aria-expanded=move || retry_open.get().to_string() aria-controls="retry-popover" id="conn-pill" data-testid="conn-pill" on:click=move |_| retry_open.update(|o| *o = !*o)>
                            <Icon name="refresh"/>
                            {move || format!("Reconnecting in {} s", retry_left(s))}
                        </button>
                        <Show when=move || retry_open.get()>
                            <div class=burrow_ui_kit::menu_class("top-8 right-0 w-56", true) id="retry-popover" tabindex="-1" role="dialog" aria-label="Connection" data-testid="retry-popover">
                                <p class="mb-2 text-text-muted">"The connection to burrow admin dropped. It retries on its own."</p>
                                <Button label="Retry now" small=true testid="retry-now" on_click=move |_| { retry_open.set(false); api::reconnect(s); }/>
                            </div>
                        </Show>
                    </span>
                }
                .into_any()
            }
            Conn::Open => view! { <span class="sr-only" data-testid="conn-pill">{move || format!("Connected to {}", s.server_name())}</span> }.into_any(),
            Conn::Connecting => view! { <span class="text-[12.5px] text-hdr-muted" data-testid="conn-pill">"Connecting"</span> }.into_any(),
            other => view! { <span class="flex items-center gap-1.5 text-[12.5px] text-danger" data-testid="conn-pill"><Icon name="wifi-off"/>{other.label()}</span> }.into_any(),
        }
    }
}

pub(super) fn facts(s: Store) -> impl IntoView + Copy + use<> {
    move || {
        s.welcome.get()?;
        let st = s.state.get()?;
        Some(view! {
            <dl class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 px-3 py-2 text-sm [&_dt]:text-text-muted [&_dd]:font-mono [&_dd]:text-xs [&_dd]:break-all [&_dd]:whitespace-pre-line">
                <dt>"Listening"</dt><dd>{s.status.get().map(|x| x.listening.join("\n")).unwrap_or_default()}</dd>
                <dt>"Config"</dt><dd>{st.file.path.clone()}</dd>
            </dl>
        })
    }
}

pub(super) fn mobile_back(s: Store) -> impl IntoView + use<> {
    move || match s.route.get() {
        Route::Detail(sec, name) => {
            let label = crate::screens::plural(&sec);
            Some(view! {
                <div class="flex shrink-0 items-center sm:hidden" data-testid="mobile-back" data-entity=name>
                    <Link to=Route::List(sec.clone()) class="flex size-8 items-center justify-center rounded-md text-accent-ink no-underline">
                        <Icon name="chevron-left"/>
                        <span class="sr-only">{format!("Back to {label}")}</span>
                    </Link>
                </div>
            })
        }
        _ => None,
    }
}

fn failed_check(s: Store, check_state: Signal<String>) -> Option<&'static str> {
    (check_state.get() == "failed")
        .then(|| {
            s.edit.check_error.with(|e| {
                e.as_ref()
                    .map(|e| burrow_admin_logic::wording::check_failed(e.code))
            })
        })
        .flatten()
}

pub(super) fn review_or_read_only(
    s: Store,
    read_only: Memo<bool>,
    why_open: RwSignal<bool>,
    pending: Memo<usize>,
    check_state: Signal<String>,
) -> impl IntoView + use<> {
    move || {
        if read_only.get() {
            view! {
                <span class="relative">
                    <button type="button" class="inline-flex h-8 cursor-pointer items-center gap-1.5 rounded-lg border border-hdr-line bg-hdr-raised px-2.5 text-[12.5px] font-semibold text-hdr-text dark:border-hdr-raised-2" aria-haspopup="true" aria-expanded=move || why_open.get().to_string() aria-controls="read-only-why" id="read-only-tag" data-testid="read-only-tag" on:click=move |_| why_open.update(|o| *o = !*o)>
                        <Icon name="eye" small=true/>
                        "Read-only"
                    </button>
                    <Show when=move || why_open.get()>
                        <div class=burrow_ui_kit::menu_class("top-8 right-0 w-80", true) id="read-only-why" tabindex="-1" role="dialog" aria-label="Why this is read-only" data-testid="read-only-why">
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
                    {move || match failed_check(s, check_state) {
                        Some(why) => view! {
                            <span class="inline-flex items-center gap-1 rounded-sm bg-danger px-1.5 text-[11.5px] text-danger-ink" title=why data-testid="review-count">
                                <Icon name=Tone::Danger.icon() small=true/>{pending.get()}
                            </span>
                            <span class="sr-only" data-testid="review-failed">{why}</span>
                        }.into_any(),
                        None => view! { <span class="rounded-sm bg-white/25 px-1.5 text-[11.5px] dark:bg-black/18" data-testid="review-count">{pending.get()}</span> }.into_any(),
                    }}
                </Link>
            }
            .into_any()
        } else {
            ().into_any()
        }
    }
}

pub(super) fn avatar_menu(
    s: Store,
    theme: RwSignal<String>,
    avatar_open: RwSignal<bool>,
    read_only: Memo<bool>,
    you: Memo<Option<(String, &'static str)>>,
) -> impl IntoView + use<> {
    let dot = move || match s.conn.get() {
        Conn::Open if read_only.get() => "bg-text-faint",
        Conn::Open => "bg-success",
        Conn::Reconnecting { .. } | Conn::Connecting => "bg-warning",
        _ => "bg-danger",
    };
    let conn_line = move || match s.conn.get() {
        Conn::Open => format!("Connected to {}", s.server_name()),
        Conn::Reconnecting { .. } => format!("Reconnecting to {}", s.server_name()),
        other => other.label().to_string(),
    };
    view! {
    <div class=burrow_ui_kit::menu_class("top-10 right-0 w-64", false) id="avatar-menu" tabindex="-1" data-testid="avatar-menu">
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
                            on:click=move |_| { set_theme(theme, t); avatar_open.set(false); }
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
    }
}
