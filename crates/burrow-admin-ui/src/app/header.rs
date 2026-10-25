use burrow_admin_proto::msg::AdminLevel;
use burrow_config::admin_schema::AdminEnum;
use burrow_config::{ClientEntry, Entry, ServiceEntry, VisitorEntry};
use burrow_ui_kit::Icon;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::route::{Link, Route};
use crate::store::{Conn, Store, store};

use super::menus::{avatar_menu, conn_state, facts, mobile_back, review_or_read_only};
use super::{THEME_KEY, apply_theme, saved_theme};

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

#[component]
pub(super) fn Header() -> impl IntoView {
    let s = store();
    let nav = nav_links(s);
    let menu_open = RwSignal::new(false);
    let avatar_open = RwSignal::new(false);
    let theme = RwSignal::new(saved_theme());
    let pending = Memo::new(move |_| {
        if s.route.with(|r| *r == Route::Review) {
            s.edit.review_count()
        } else {
            s.edit.pending()
        }
    });
    let check_state = check_state(s);
    keep_current_tab_visible(s);
    let read_only = Memo::new(move |_| s.conn.get() == Conn::Open && !s.can_write());
    let why_open = RwSignal::new(false);
    let retry_open = RwSignal::new(false);
    popover(menu_open, "server-switcher", "server-menu");
    popover(avatar_open, "avatar", "avatar-menu");
    Effect::new(move |_| {
        s.route.track();
        s.search_open.track();
        avatar_open.set(false);
        menu_open.set(false);
    });
    popover(why_open, "read-only-tag", "read-only-why");
    popover(retry_open, "conn-pill", "retry-popover");
    let conn_state = conn_state(s, retry_open);
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
    let facts = facts(s);
    view! {
        <header class="sticky top-0 z-30 grid grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] items-center gap-4 border-b border-hdr-line bg-hdr px-5 text-hdr-text max-xl:grid-cols-[minmax(0,1fr)_auto] max-xl:pt-3 max-sm:gap-x-2 max-sm:px-3 max-sm:pt-2 xl:h-16" data-testid="header">
            {mobile_back(s)}
            <div class=move || format!("relative flex min-w-0 items-center gap-2.5{}", if matches!(s.route.get(), Route::Detail(..)) { " max-sm:hidden" } else { "" })>
                <span class="flex size-[30px] shrink-0 items-center justify-center rounded-md bg-gradient-to-b from-logo-from to-logo-to text-white" aria-hidden="true"><Icon name="logo" large=true/></span>
                <button type="button" class="flex min-w-0 cursor-pointer items-center gap-2 rounded-md px-1.5 py-1 hover:bg-hdr-raised" aria-haspopup="true" aria-expanded=move || menu_open.get().to_string() aria-controls="server-menu" id="server-switcher" data-testid="server-switcher" title=move || s.status.with(|x| x.as_ref().map(|x| x.listening.join(", ")).unwrap_or_default()) on:click=move |_| menu_open.update(|o| *o = !*o)>
                    <span class="flex min-w-0 flex-col text-left">
                        <span class="truncate font-semibold text-hdr-on" data-testid="server-line">{move || s.server_name()}</span>
                    </span>
                    <Icon name="chevron-down" small=true/>
                </button>
                <Show when=move || menu_open.get()>
                    <div class=burrow_ui_kit::menu_class("top-11 left-10 w-80", false) id="server-menu" tabindex="-1" data-testid="server-menu" on:click=move |_| menu_open.set(false)>
                        {facts}
                        <div class="border-t border-border p-1">
                            <Link to=Route::About class="flex items-center gap-2 rounded-md px-2 py-1.5 text-sm no-underline hover:bg-surface-3">"About this server"</Link>
                        </div>
                    </div>
                </Show>
            </div>
            <nav aria-label="Sections" class="flex gap-0.5 rounded-[11px] border border-hdr-line bg-hdr-raised p-[3px] max-xl:col-span-2 max-xl:row-start-2 max-xl:mb-3 max-xl:justify-self-center max-sm:mb-2 max-sm:max-w-full max-sm:justify-self-stretch max-sm:overflow-x-auto max-sm:rounded-none max-sm:border-0 max-sm:bg-transparent max-sm:p-0 max-sm:pr-8 max-sm:[scrollbar-width:none] max-sm:[mask-image:linear-gradient(to_right,black_calc(100%-2rem),transparent)] dark:border-hdr-raised-2">
                {nav}
            </nav>
            <div class="flex items-center justify-end gap-3" data-testid="header-right">
                <button type="button" class="flex size-8 cursor-pointer items-center justify-center rounded-lg text-hdr-text outline-none hover:bg-hdr-raised focus-visible:bg-hdr-raised" aria-label="Search (/)" title="Search (/)" data-testid="search-open" on:click=move |_| s.search_open.set(true)>
                    <Icon name="search"/>
                </button>
                <button
                    type="button"
                    class="flex size-8 cursor-pointer items-center justify-center rounded-lg text-hdr-text outline-none hover:bg-hdr-raised focus-visible:bg-hdr-raised max-[1199px]:hidden"
                    aria-label=move || { let (now, next, _) = next_theme(); format!("Theme: {now}. Switch to {next}") }
                    title=move || { let (now, next, _) = next_theme(); format!("Theme: {now}. Switch to {next}") }
                    data-testid="theme-toggle"
                    data-theme=move || if theme.get().is_empty() { "system".to_string() } else { theme.get() }
                    on:click=move |_| { let (_, _, t) = next_theme(); set_theme(theme, t); }
                >
                    {move || match theme.get().as_str() {
                        "light" => view! { <Icon name="sun"/> }.into_any(),
                        "dark" => view! { <Icon name="moon"/> }.into_any(),
                        _ => view! { <Icon name="monitor"/> }.into_any(),
                    }}
                </button>
                {conn_state}
                {review_or_read_only(s, read_only, why_open, pending, check_state)}
                {move || s.edit.dirty().then(|| view! { <span class="hidden" data-testid="check-state" data-state=check_state></span> })}
                <div class="relative flex items-center gap-2">
                    <button type="button" class="relative flex size-[30px] cursor-pointer items-center justify-center rounded-full bg-hdr-raised-2 text-xs font-bold text-hdr-on" aria-label=move || format!("Account {}", you.get().map(|y| y.0).unwrap_or_default()) title=conn_line aria-haspopup="true" aria-expanded=move || avatar_open.get().to_string() aria-controls="avatar-menu" id="avatar" data-testid="avatar" on:click=move |_| avatar_open.update(|o| *o = !*o)>
                        {move || you.get().map(|y| initials(&y.0)).unwrap_or_default()}
                        <span class=move || format!("absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full ring-2 ring-hdr {}", dot())></span>
                    </button>
                    <span class="sr-only" data-testid="you-level">{move || you.get().map(|y| format!("{} ({})", y.0, y.1)).unwrap_or_default()}</span>
                    <Show when=move || avatar_open.get()>
                        {avatar_menu(s, theme, avatar_open, read_only, you)}
                    </Show>
                </div>
            </div>
        </header>
    }
}

pub(super) fn set_theme(theme: RwSignal<String>, t: &'static str) {
    theme.set(t.to_string());
    apply_theme(t);
    if let Some(st) = window().local_storage().ok().flatten() {
        let _ = st.set_item(THEME_KEY, t);
    }
}

fn section_tab(section: &'static str) -> (Route, &'static str, &'static str) {
    (
        Route::List(section.into()),
        crate::screens::plural(section),
        crate::screens::entity_icon(section),
    )
}

fn nav_links(s: Store) -> impl IntoView + use<> {
    let tabs: [(Route, &str, &str); 6] = [
        (Route::Overview, "Overview", "overview"),
        section_tab(ServiceEntry::section()),
        section_tab(ClientEntry::section()),
        section_tab(VisitorEntry::section()),
        (Route::Settings, "Settings", "settings"),
        (Route::Activity(None), "Activity", "activity"),
    ];
    tabs
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
        .collect_view()
}

fn check_state(s: Store) -> Signal<String> {
    Signal::derive(move || {
        let e = s.edit;
        let errors = e.local.with(|l| l.len())
            + e.related.with(|r| r.len())
            + e.plan.with(|p| p.as_ref().map(|p| p.errors()).unwrap_or(0));
        let waiting = e.plan.with(Option::is_none)
            && e.check_error.with(Option::is_none)
            && e.conflict.with(Option::is_none)
            && e.dirty();
        if e.checking.get() || waiting {
            "checking".to_string()
        } else if errors > 0 {
            "errors".to_string()
        } else {
            "ok".to_string()
        }
    })
}

fn show_current_tab() {
    let overflows = document()
        .query_selector("nav[aria-label=Sections]")
        .ok()
        .flatten()
        .is_some_and(|n| n.scroll_width() > n.client_width());
    if overflows
        && let Ok(Some(el)) =
            document().query_selector("nav[aria-label=Sections] [aria-current=page]")
    {
        let opts = web_sys::ScrollIntoViewOptions::new();
        opts.set_block(web_sys::ScrollLogicalPosition::Nearest);
        opts.set_inline(web_sys::ScrollLogicalPosition::Center);
        el.scroll_into_view_with_scroll_into_view_options(&opts);
    }
}

fn keep_current_tab_visible(s: Store) {
    Effect::new(move |_| {
        s.route.track();
        request_animation_frame(show_current_tab);
    });
    let resized = window_event_listener(leptos::ev::resize, |_| show_current_tab());
    on_cleanup(move || resized.remove());
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

pub(super) fn read_only_reason(s: Store) -> (String, String) {
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
