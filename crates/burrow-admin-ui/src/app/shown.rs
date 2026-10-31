use leptos::prelude::*;

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
