use std::cell::Cell;
use std::rc::Rc;

use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use super::range::Viewport;

#[derive(Clone, Copy)]
pub enum Scroller {
    Itself(NodeRef<html::Div>),
    Closest(NodeRef<html::Div>, &'static str),
}

#[derive(Clone, Copy)]
pub struct Tracked {
    pub view: ReadSignal<Viewport>,
    pub ready: ReadSignal<bool>,
    set: WriteSignal<Viewport>,
    el: StoredValue<Option<web_sys::HtmlElement>, LocalStorage>,
}

impl Tracked {
    pub fn scroll_to(&self, top: f64) {
        let Some(el) = self.el.get_value() else {
            return;
        };
        el.set_scroll_top(top.max(0.0) as i32);
        self.set.set(Viewport {
            top: f64::from(el.scroll_top()),
            height: f64::from(el.client_height()),
        });
    }

    pub fn element(&self) -> Option<web_sys::HtmlElement> {
        self.el.get_value()
    }
}

type Watch = (
    web_sys::HtmlElement,
    Closure<dyn FnMut()>,
    Option<web_sys::ResizeObserver>,
);

fn find(scroller: Scroller) -> Option<web_sys::HtmlElement> {
    match scroller {
        Scroller::Itself(me) => me.get().map(Into::into),
        Scroller::Closest(me, selector) => {
            let me: web_sys::HtmlElement = me.get()?.into();
            me.closest(selector)
                .ok()
                .flatten()
                .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
        }
    }
}

fn stop(watch: StoredValue<Option<Watch>, LocalStorage>) {
    watch.try_with_value(|w| {
        if let Some((el, on, size)) = w {
            let _ = el.remove_event_listener_with_callback("scroll", on.as_ref().unchecked_ref());
            if let Some(o) = size {
                o.disconnect();
            }
        }
    });
}

pub fn track(scroller: Scroller) -> Tracked {
    let (view, set) = signal(Viewport::default());
    let (ready, set_ready) = signal(false);
    let el: StoredValue<Option<web_sys::HtmlElement>, LocalStorage> = StoredValue::new_local(None);
    let watch: StoredValue<Option<Watch>, LocalStorage> = StoredValue::new_local(None);
    Effect::new(move |_| {
        let Some(list) = find(scroller) else {
            return;
        };
        stop(watch);
        let seen = list.clone();
        let read = move || {
            let now = Viewport {
                top: f64::from(seen.scroll_top()),
                height: f64::from(seen.client_height()),
            };
            if view.try_get_untracked().is_some_and(|v| v != now) {
                set.set(now);
            }
        };
        read();
        let waiting = Rc::new(Cell::new(false));
        let on = Closure::<dyn FnMut()>::new(move || {
            if waiting.replace(true) {
                return;
            }
            let (read, waiting) = (read.clone(), waiting.clone());
            request_animation_frame(move || {
                waiting.set(false);
                read();
            });
        });
        let _ = list.add_event_listener_with_callback("scroll", on.as_ref().unchecked_ref());
        let size = web_sys::ResizeObserver::new(on.as_ref().unchecked_ref()).ok();
        if let Some(o) = &size {
            o.observe(&list);
        }
        el.set_value(Some(list.clone()));
        set_ready.set(true);
        watch.set_value(Some((list, on, size)));
    });
    on_cleanup(move || stop(watch));
    Tracked {
        view,
        ready,
        set,
        el,
    }
}
