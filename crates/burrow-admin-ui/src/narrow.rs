use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

const CARDS_BELOW: i32 = 760;

type Watch = (web_sys::ResizeObserver, Closure<dyn FnMut()>);

pub(crate) fn narrow(node: NodeRef<html::Div>) -> ReadSignal<bool> {
    let (now, set) = signal(!crate::screens::wide());
    let watch: StoredValue<Option<Watch>, LocalStorage> = StoredValue::new_local(None);
    Effect::new(move |_| {
        let Some(el) = node.get() else {
            return;
        };
        stop(watch);
        let el: web_sys::HtmlElement = el.into();
        let seen = el.clone();
        let measure = move || {
            let n = seen.client_width() < CARDS_BELOW;
            if now.try_get_untracked().is_some_and(|o| o != n) {
                set.set(n);
            }
        };
        measure();
        let on_resize = Closure::<dyn FnMut()>::new(measure);
        if let Ok(o) = web_sys::ResizeObserver::new(on_resize.as_ref().unchecked_ref()) {
            o.observe(&el);
            watch.set_value(Some((o, on_resize)));
        }
    });
    on_cleanup(move || stop(watch));
    now
}

fn stop(watch: StoredValue<Option<Watch>, LocalStorage>) {
    watch.try_with_value(|w| {
        if let Some((o, _)) = w {
            o.disconnect();
        }
    });
}
