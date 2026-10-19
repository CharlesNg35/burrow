use leptos::prelude::*;
use wasm_bindgen::JsValue;

use crate::store::store;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Overview,
    List(String),
    Detail(String, String),
    Settings,
    Activity,
    About,
    NotFound(String),
}

pub const SECTIONS: [&str; 3] = ["services", "clients", "visitors"];

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
            ["activity"] => Route::Activity,
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
            Route::Activity => "/activity".into(),
            Route::About => "/about".into(),
            Route::NotFound(p) => p.clone(),
        }
    }

    pub fn section(&self) -> &str {
        match self {
            Route::Overview => "overview",
            Route::List(s) | Route::Detail(s, _) => s,
            Route::Settings => "settings",
            Route::Activity => "activity",
            Route::About => "about",
            Route::NotFound(_) => "",
        }
    }
}

pub fn current() -> Route {
    let path = window().location().pathname().unwrap_or_default();
    Route::parse(&path)
}

pub fn go(route: Route) {
    let s = store();
    if s.route.get_untracked() == route {
        return;
    }
    if let Ok(h) = window().history() {
        let _ = h.push_state_with_url(&JsValue::NULL, "", Some(&route.path()));
    }
    s.route.set(route);
    window().scroll_to_with_x_and_y(0.0, 0.0);
}

#[component]
pub fn Link(
    to: Route,
    #[prop(optional, into)] class: String,
    #[prop(optional)] testid: &'static str,
    children: Children,
) -> impl IntoView {
    let href = to.path();
    view! {
        <a
            href=href
            class=class
            data-testid=(!testid.is_empty()).then_some(testid)
            on:click=move |e| {
                if e.ctrl_key() || e.meta_key() || e.shift_key() || e.button() != 0 {
                    return;
                }
                e.prevent_default();
                go(to.clone());
            }
        >
            {children()}
        </a>
    }
}
