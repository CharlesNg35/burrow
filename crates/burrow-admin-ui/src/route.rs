use burrow_config::{ClientEntry, Entry, ServiceEntry, VisitorEntry};
use leptos::prelude::*;
use wasm_bindgen::JsValue;

use crate::store::store;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Route {
    #[default]
    Overview,
    List(String),
    Detail(String, String),
    Settings,
    Activity(Option<String>),
    Review,
    About,
    NotFound(String),
}

pub const FILTER_QUERY: &str = "filter";
pub const WEBSITES_FILTER: &str = "websites";
pub const CARD_QUERY: &str = "card";
pub const TAB_QUERY: &str = "tab";

pub const SECTIONS: [&str; 3] = [
    ServiceEntry::SECTION,
    ClientEntry::SECTION,
    VisitorEntry::SECTION,
];

fn decode(s: &str) -> String {
    js_sys::decode_uri_component(s)
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_else(|| s.to_string())
}

fn encode(s: &str) -> String {
    String::from(js_sys::encode_uri_component(s))
}

impl Route {
    pub fn parse(path: &str) -> Route {
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        match parts.as_slice() {
            [] => Route::Overview,
            ["settings"] => Route::Settings,
            ["activity"] => Route::Activity(None),
            ["activity", rev] => Route::Activity(Some(decode(rev))),
            ["review"] => Route::Review,
            ["about"] => Route::About,
            [s] if SECTIONS.contains(s) => Route::List(s.to_string()),
            [s, name] if SECTIONS.contains(s) => Route::Detail(s.to_string(), decode(name)),
            _ => Route::NotFound(path.to_string()),
        }
    }

    pub fn path(&self) -> String {
        match self {
            Route::Overview => "/".into(),
            Route::List(s) => format!("/{s}"),
            Route::Detail(s, n) => format!("/{s}/{}", encode(n)),
            Route::Settings => "/settings".into(),
            Route::Activity(None) => "/activity".into(),
            Route::Activity(Some(r)) => format!("/activity/{}", encode(r)),
            Route::Review => "/review".into(),
            Route::About => "/about".into(),
            Route::NotFound(p) => p.clone(),
        }
    }

    pub fn section(&self) -> &str {
        match self {
            Route::Overview => "overview",
            Route::List(s) | Route::Detail(s, _) => s,
            Route::Settings => "settings",
            Route::Activity(_) => "activity",
            Route::Review => "review",
            Route::About => "about",
            Route::NotFound(_) => "",
        }
    }
}

fn split_run(path: &str) -> (&str, &str) {
    let Some(rest) = path.strip_prefix(burrow_admin_proto::local::RUN_PREFIX) else {
        return ("", path);
    };
    match rest.find('/') {
        Some(i) => {
            let at = i + burrow_admin_proto::local::RUN_PREFIX.len();
            (&path[..at], &path[at..])
        }
        None => (path, "/"),
    }
}

pub fn run_prefix() -> String {
    let path = window().location().pathname().unwrap_or_default();
    split_run(&path).0.to_string()
}

pub fn href(route: &Route) -> String {
    format!("{}{}", run_prefix(), route.path())
}

pub fn current() -> Route {
    let path = window().location().pathname().unwrap_or_default();
    Route::parse(split_run(&path).1)
}

pub fn replace(route: Route) {
    let s = store();
    let search = window().location().search().unwrap_or_default();
    if let Ok(h) = window().history() {
        let _ = h.replace_state_with_url(
            &JsValue::NULL,
            "",
            Some(&format!("{}{search}", href(&route))),
        );
    }
    if s.route.get_untracked() != route {
        s.route.set(route);
    }
}

pub fn query(key: &str) -> Option<String> {
    let search = window().location().search().unwrap_or_default();
    search
        .trim_start_matches('?')
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| decode(v))
}

pub fn set_query(key: &str, value: &str) {
    let path = window().location().pathname().unwrap_or_default();
    if let Ok(h) = window().history() {
        let _ = h.replace_state_with_url(
            &JsValue::NULL,
            "",
            Some(&format!("{path}?{key}={}", encode(value))),
        );
    }
}

pub fn go(route: Route) {
    go_in(store(), route);
}

pub type Query = Option<(&'static str, &'static str)>;

fn href_with(route: &Route, query: Query) -> String {
    match query {
        Some((k, v)) => format!("{}?{k}={}", href(route), encode(v)),
        None => href(route),
    }
}

pub fn go_query(route: Route, query: Query) {
    go_with(store(), route, query);
}

pub fn go_in(s: crate::store::Store, route: Route) {
    go_with(s, route, None);
}

fn go_with(s: crate::store::Store, route: Route, query: Query) {
    if s.route.get_untracked() == route && query.is_none() {
        return;
    }
    if let Ok(h) = window().history() {
        let _ = h.push_state_with_url(&JsValue::NULL, "", Some(&href_with(&route, query)));
    }
    let was = s.route.get_untracked();
    if was != Route::Review {
        s.prev.set(was);
    }
    s.route.set(route);
    s.query_seen.update(|n| *n += 1);
    window().scroll_to_with_x_and_y(0.0, 0.0);
}

#[component]
pub fn Link(
    to: Route,
    #[prop(optional, into)] class: String,
    #[prop(optional)] testid: &'static str,
    #[prop(optional, into)] current: Option<Signal<bool>>,
    #[prop(optional, into)] state: Option<Signal<String>>,
    #[prop(optional)] query: Query,
    children: Children,
) -> impl IntoView {
    let href = href_with(&to, query);
    view! {
        <a
            href=href
            class=class
            aria-current=move || current.and_then(|c| c.get().then_some("page"))
            data-testid=(!testid.is_empty()).then_some(testid)
            data-state=move || state.map(|s| s.get())
            on:click=move |e| {
                if e.ctrl_key() || e.meta_key() || e.shift_key() || e.button() != 0 {
                    return;
                }
                e.prevent_default();
                go_with(store(), to.clone(), query);
            }
        >
            {children()}
        </a>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_run_prefix_is_split_off_the_path() {
        let run = "/r-0123456789abcdef0123456789abcdef";
        assert_eq!(
            split_run(&format!("{run}/services/web")),
            (run, "/services/web")
        );
        assert_eq!(split_run(&format!("{run}/")), (run, "/"));
        assert_eq!(split_run(run), (run, "/"));
        assert_eq!(split_run("/services/web"), ("", "/services/web"));
        assert_eq!(split_run("/"), ("", "/"));
    }
}
