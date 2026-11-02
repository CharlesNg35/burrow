use leptos::prelude::*;
use wasm_bindgen::JsCast;

pub(crate) fn shown_while<T: Send + Sync + 'static>(
    open: RwSignal<T>,
    is_open: fn(&T) -> bool,
    close: fn(&mut T),
) -> RwSignal<bool> {
    let shown = RwSignal::new(open.with_untracked(is_open));
    Effect::new(move |_| {
        let now = open.with(is_open);
        if shown.get_untracked() != now {
            shown.set(now);
        }
    });
    Effect::new(move |_| {
        if !shown.get() && open.with_untracked(is_open) {
            open.update(close);
        }
    });
    shown
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
