//! Focus and scroll-into-view helpers that move the page only as far as needed: never on focus, the least distance on a reveal.

pub fn focus_still(el: &web_sys::HtmlElement) {
    let opts = web_sys::FocusOptions::new();
    opts.set_prevent_scroll(true);
    let _ = el.focus_with_options(&opts);
}

pub fn bring_near(el: &web_sys::Element) {
    let opts = web_sys::ScrollIntoViewOptions::new();
    opts.set_block(web_sys::ScrollLogicalPosition::Nearest);
    opts.set_inline(web_sys::ScrollLogicalPosition::Nearest);
    el.scroll_into_view_with_scroll_into_view_options(&opts);
}

pub fn focus_in_view(el: &web_sys::HtmlElement) {
    focus_still(el);
    bring_near(el);
}
