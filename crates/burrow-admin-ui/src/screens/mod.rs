mod about;
pub mod activity;
pub mod create;
pub mod entity;
pub mod flow;
mod list;
mod local_ca;
mod overview;
pub mod paths;
pub mod placement;
mod settings;

use burrow_ui_kit::{EmptyState, Icon, Tone, button_class};
use leptos::prelude::*;

use crate::route::{Link, Route};
use crate::store::store;

pub(crate) use list::{ListCard, ListPane, NewChoice};

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

#[derive(Debug, Clone, PartialEq, Default)]
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
    pub line3: String,
    pub aside: String,
    pub title_tag: Option<(String, Tone)>,
    pub terms: String,
}

impl Row {
    fn matches(&self, q: &str) -> bool {
        q.is_empty()
            || self.title.to_lowercase().contains(q)
            || self.line2.to_lowercase().contains(q)
            || self.meta.to_lowercase().contains(q)
            || self.terms.to_lowercase().contains(q)
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
                <div class="mb-3 max-sm:hidden lg:hidden">
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

pub fn clear_of_bar() {
    use wasm_bindgen::JsCast;
    let doc = document();
    let Some(root) = doc
        .document_element()
        .and_then(|r| r.dyn_into::<web_sys::HtmlElement>().ok())
    else {
        return;
    };
    let Some(bar) = doc.query_selector("[data-sticky-bar]").ok().flatten() else {
        let _ = root.style().remove_property("scroll-padding-bottom");
        return;
    };
    let h = window()
        .inner_height()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let top = bar.get_bounding_client_rect().top();
    let room = (h - top + 16.0).clamp(16.0, h / 2.0);
    let _ = root
        .style()
        .set_property("scroll-padding-bottom", &format!("{room}px"));
    let Some(active) = doc.active_element() else {
        return;
    };
    if bar.contains(Some(&active)) || active.tag_name() == "BODY" {
        return;
    }
    let row = active
        .closest("[data-schema]")
        .ok()
        .flatten()
        .filter(|r| r.get_bounding_client_rect().height() < h / 2.0)
        .unwrap_or(active);
    let r = row.get_bounding_client_rect();
    let limit = top;
    if r.bottom() > limit && r.top() < h {
        let by = (r.bottom() - limit + 12.0).min(r.top() - 80.0).max(0.0);
        window().scroll_by_with_x_and_y(0.0, by);
    }
}

pub fn master_detail(list: AnyView, detail: AnyView) -> AnyView {
    view! {
        <div class="grid min-h-[calc(100vh-var(--hdr-h,64px))] items-start lg:grid-cols-[380px_minmax(0,1fr)] lg:bg-[linear-gradient(to_right,var(--color-surface)_379px,var(--color-border)_379px,var(--color-border)_380px,transparent_380px)]">
            {list}
            {detail}
        </div>
    }
    .into_any()
}

pub fn no_token() -> impl IntoView {
    let port = window().location().port().unwrap_or_default();
    let step = |n: &'static str, text: &'static str| {
        view! {
            <li class="flex items-center gap-3 border-b border-border py-2.5 text-sm last:border-b-0">
                <span class="flex size-5 shrink-0 items-center justify-center rounded-[5px] bg-accent-weak text-[11px] font-semibold text-accent-ink">{n}</span>
                <span>{text}</span>
            </li>
        }
    };
    view! {
        <div class="flex min-h-screen flex-col bg-bg p-6" data-testid="no-token">
            <div class="flex flex-1 items-center justify-center">
                <div class="grid w-full max-w-[1000px] overflow-hidden rounded-[16px] border border-border bg-surface shadow-[0_12px_40px_rgba(15,23,42,0.10)] md:grid-cols-[416px_minmax(0,1fr)]">
                    <div class="p-9">
                        <div class="mb-7 flex items-center gap-2.5">
                            <span class="flex size-[30px] items-center justify-center rounded-md bg-gradient-to-b from-logo-from to-logo-to text-white" aria-hidden="true"><Icon name="logo" large=true/></span>
                            <span class="font-semibold">"burrow admin"</span>
                        </div>
                        <h1 class="mb-3 text-xl font-bold tracking-tight">"Open the link from your terminal"</h1>
                        <p class="mb-5 text-sm leading-relaxed text-text-muted">"This page needs the link that "<code class="font-mono text-[13px]">"burrow admin"</code>" printed when it started. The link keeps other programs on this machine out of your admin session."</p>
                        <ol>
                            {step("1", "Find the terminal where burrow admin runs.")}
                            {step("2", "Open the link it printed while it runs.")}
                            {step("3", "No link? Run it again with --print-url.")}
                        </ol>
                    </div>
                    <div class="flex flex-col justify-center bg-code p-6 font-mono text-xs leading-relaxed text-code-text">
                        <div class="mb-4 flex items-center gap-1.5" aria-hidden="true">
                            <span class="size-2 rounded-full bg-[#ef4444]"></span>
                            <span class="size-2 rounded-full bg-[#f59e0b]"></span>
                            <span class="size-2 rounded-full bg-[#22c55e]"></span>
                            <span class="ml-3 font-sans text-code-text/55">"terminal"</span>
                        </div>
                        <pre class="overflow-x-auto whitespace-pre-wrap break-all">
                            <span class="text-code-text/55">"$ "</span>"burrow admin -c visitor.yaml --print-url\n"
                            <span class="text-code-text/55">{format!("burrow admin for … listens on http://127.0.0.1:{port}\nopen: ")}</span>
                            <span class="text-[#7cb7ff] underline">{format!("http://127.0.0.1:{port}/{}…", burrow_admin_proto::local::TOKEN_FRAGMENT)}</span>
                            <span class="text-code-text/55">"\nstop it with Ctrl-C"</span>
                        </pre>
                    </div>
                </div>
            </div>
            <p class="pt-6 text-center text-xs text-text-faint">{format!("Served by burrow admin on 127.0.0.1:{port} · nothing was sent to the server")}</p>
        </div>
    }
}
