use leptos::html;
use leptos::prelude::*;

const ROW: &str = "flex min-w-0 flex-nowrap overflow-x-auto overscroll-x-contain [scrollbar-width:none] [&::-webkit-scrollbar]:hidden data-[fade=true]:[mask-image:linear-gradient(to_right,black_calc(100%-28px),transparent)]";
const FADE_PX: f64 = 28.0;

fn more_on_the_right(left: i32, width: i32, all: i32) -> bool {
    all > width + 1 && left + width < all - 1
}

fn fades(el: &web_sys::HtmlElement) -> bool {
    more_on_the_right(el.scroll_left(), el.client_width(), el.scroll_width())
}

fn show(el: &web_sys::HtmlElement, selector: &str) {
    if el.scroll_width() <= el.client_width() {
        return;
    }
    if let Ok(Some(on)) = el.query_selector(selector) {
        let (row, item) = (el.get_bounding_client_rect(), on.get_bounding_client_rect());
        let by = sideways(row.left(), row.right() - FADE_PX, item.left(), item.right());
        if by != 0.0 {
            el.set_scroll_left(el.scroll_left() + by.round() as i32);
        }
    }
}

fn sideways(row_left: f64, row_right: f64, left: f64, right: f64) -> f64 {
    if left < row_left {
        left - row_left
    } else if right > row_right {
        right - row_right
    } else {
        0.0
    }
}

pub fn active_item(attr: &'static str, active: Signal<&'static str>) -> Signal<String> {
    Signal::derive(move || format!("[{attr}=\"{}\"]", active.get()))
}

#[component]
pub fn ScrollRow(
    #[prop(into)] active: Signal<String>,
    #[prop(optional)] class: &'static str,
    #[prop(optional)] role: &'static str,
    #[prop(optional)] testid: &'static str,
    children: Children,
) -> impl IntoView {
    let row: NodeRef<html::Div> = NodeRef::new();
    let fade = RwSignal::new(false);
    let fit = move |selector: Option<String>| {
        let Some(el) = row.try_get_untracked().flatten() else {
            return;
        };
        let el: web_sys::HtmlElement = el.into();
        if let Some(selector) = selector {
            show(&el, &selector);
        }
        let now = fades(&el);
        if fade.get_untracked() != now {
            fade.set(now);
        }
    };
    Effect::new(move |_| {
        let selector = active.get();
        request_animation_frame(move || fit(Some(selector)));
    });
    let resize = window_event_listener(leptos::ev::resize, move |_| {
        fit(Some(active.get_untracked()))
    });
    on_cleanup(move || resize.remove());
    view! {
        <div
            class=format!("{ROW} {class}")
            role=(!role.is_empty()).then_some(role)
            data-testid=(!testid.is_empty()).then_some(testid)
            data-fade=move || fade.get().to_string()
            node_ref=row
            on:scroll=move |_| fit(None)
        >
            {children()}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_edge_fades_only_while_more_of_the_row_is_hidden_on_the_right() {
        assert!(!more_on_the_right(0, 300, 300), "everything fits");
        assert!(
            !more_on_the_right(0, 300, 301),
            "a pixel of rounding is no overflow"
        );
        assert!(more_on_the_right(0, 300, 420));
        assert!(more_on_the_right(60, 300, 420));
        assert!(!more_on_the_right(120, 300, 420), "scrolled to the end");
    }

    #[test]
    fn the_active_item_comes_into_view_by_scrolling_only_the_row() {
        assert_eq!(sideways(0.0, 300.0, 20.0, 80.0), 0.0);
        assert_eq!(sideways(0.0, 300.0, 280.0, 340.0), 40.0);
        assert_eq!(sideways(10.0, 300.0, -30.0, 30.0), -40.0);
    }

    #[test]
    fn the_row_never_wraps_and_scrolls_sideways_without_a_scrollbar() {
        for class in [
            "flex-nowrap",
            "overflow-x-auto",
            "[scrollbar-width:none]",
            "data-[fade=true]:[mask-image",
        ] {
            assert!(ROW.contains(class), "{class}");
        }
        assert!(!ROW.contains("flex-wrap "));
    }
}
