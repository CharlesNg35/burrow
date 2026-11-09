use leptos::prelude::*;

use crate::{Icon, Kbd};

pub fn word_at(lower: &str, needle: &str) -> Option<usize> {
    lower.match_indices(needle).map(|(i, _)| i).find(|&i| {
        lower[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric())
    })
}

fn lowered(label: &str) -> (String, Vec<(usize, usize)>) {
    let mut lower = String::with_capacity(label.len());
    let mut starts = Vec::with_capacity(label.len() + 1);
    for (at, c) in label.char_indices() {
        starts.push((lower.len(), at));
        lower.extend(c.to_lowercase());
    }
    starts.push((lower.len(), label.len()));
    (lower, starts)
}

fn found_in(label: &str, needle: &str) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return None;
    }
    let (lower, starts) = lowered(label);
    let at = word_at(&lower, needle).or_else(|| lower.find(needle))?;
    let original = |i: usize| starts.iter().find(|(l, _)| *l == i).map(|(_, o)| *o);
    Some((original(at)?, original(at + needle.len())?))
}

pub fn marked(label: &str, needle: &str) -> AnyView {
    match found_in(label, needle) {
        Some((from, to)) => {
            let (a, b, c) = (&label[..from], &label[from..to], &label[to..]);
            view! { <span>{a.to_string()}<span class="font-bold text-accent-ink">{b.to_string()}</span>{c.to_string()}</span> }.into_any()
        }
        None => view! { <span>{label.to_string()}</span> }.into_any(),
    }
}

#[component]
pub fn PickHead(#[prop(into)] text: String) -> impl IntoView {
    view! { <li class="px-2.5 pt-2.5 pb-1 text-2xs font-bold tracking-[0.08em] text-text-faint uppercase" role="presentation">{text}</li> }
}

#[component]
pub fn PickOption(
    icon: &'static str,
    #[prop(into)] active: Signal<bool>,
    on_hover: Callback<()>,
    on_pick: Callback<()>,
    #[prop(optional, into)] id: Option<String>,
    #[prop(optional, into)] note: String,
    #[prop(optional)] testid: &'static str,
    children: Children,
) -> impl IntoView {
    view! {
        <li role="option" id=id aria-selected=move || active.get().to_string() data-testid=(!testid.is_empty()).then_some(testid)>
            <button
                type="button"
                class="flex w-full min-w-0 cursor-pointer items-center gap-2.5 rounded-md px-2.5 py-2 text-left text-sm"
                class:bg-surface-3=move || active.get()
                on:mouseenter=move |_| on_hover.run(())
                on:click=move |_| on_pick.run(())
            >
                <span class="shrink-0 text-text-muted"><Icon name=icon/></span>
                <span class="min-w-0 truncate">{children()}</span>
                <span class="ml-auto min-w-0 truncate text-xs text-text-muted">{note}</span>
                {move || active.get().then(|| view! { <span class="shrink-0 max-sm:hidden"><Kbd text="Enter"/></span> })}
            </button>
        </li>
    }
}

#[cfg(test)]
mod tests {
    use super::{found_in, word_at};

    #[test]
    fn the_highlight_lands_on_the_matched_letters_whatever_their_lowercase_length() {
        let label = "İstanbul web";
        let (from, to) = found_in(label, "web").unwrap();
        assert_eq!(&label[from..to], "web");
        let (from, to) = found_in(label, "stan").unwrap();
        assert_eq!(&label[from..to], "stan");
        assert_eq!(
            found_in("Web-17", "web").map(|(f, t)| &"Web-17"[f..t]),
            Some("Web")
        );
        assert_eq!(found_in("abc", ""), None);
        assert_eq!(found_in("abc", "x"), None);
    }

    #[test]
    fn an_action_matches_where_a_word_starts() {
        assert_eq!(word_at("rotate secret of etiolles…", "et"), Some(17));
        assert_eq!(word_at("rotate secret of home…", "et"), None);
        assert_eq!(
            word_at("rotate secret of etiolles-ai-pve…", "pve"),
            Some(29)
        );
        assert_eq!(word_at("rotate secret of home…", "secret of"), Some(7));
    }
}
