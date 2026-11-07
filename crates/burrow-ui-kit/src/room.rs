use leptos::prelude::*;
use wasm_bindgen::JsCast;

const ROOM: &str = "--toast-room";
const STACK: &str = "[data-toast-stack]";

thread_local! {
    static BARS: ArcRwSignal<u32> = ArcRwSignal::new(0);
}

pub(crate) fn bars_changed() {
    BARS.with(|b| b.update(|n| *n = n.wrapping_add(1)));
}

pub(crate) fn track_bars() {
    BARS.with(|b| b.track());
}

#[derive(Default)]
pub(crate) struct Room {
    page: String,
    px: f64,
}

impl Room {
    pub(crate) fn held(&self) -> bool {
        self.px > 0.0
    }

    pub(crate) fn keep(&mut self, tall: f64, route: &str) {
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
            return;
        };
        let bar = matches!(doc.query_selector("[data-sticky-bar]"), Ok(Some(_)));
        let page = format!("{route} {bar}");
        let wanted = if tall > 0.0 {
            tall + if bar { 8.0 } else { 24.0 }
        } else {
            0.0
        };
        self.px = if page == self.page {
            self.px.max(wanted)
        } else {
            wanted
        };
        self.page = page;
        let Some(root) = doc
            .document_element()
            .and_then(|r| r.dyn_into::<web_sys::HtmlElement>().ok())
        else {
            return;
        };
        let style = root.style();
        let value = if self.held() {
            format!("{}px", self.px)
        } else {
            String::new()
        };
        if style.get_property_value(ROOM).unwrap_or_default() == value {
            return;
        }
        let _ = if value.is_empty() {
            style.remove_property(ROOM).map(drop)
        } else {
            style.set_property(ROOM, &value)
        };
    }
}

pub fn cover_top(over: Option<(f64, f64)>) -> Option<f64> {
    let stack = web_sys::window()?
        .document()?
        .query_selector(STACK)
        .ok()
        .flatten()
        .map(|e| e.get_bounding_client_rect())
        .filter(|r| r.height() > 0.0 && r.top() < crate::bar::view_height())
        .filter(|r| over.is_none_or(|(left, right)| r.left() < right && left < r.right()))
        .map(|r| r.top());
    match (crate::bar::bar_top(), stack) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}
