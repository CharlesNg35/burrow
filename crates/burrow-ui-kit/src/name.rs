use leptos::prelude::*;

use crate::pick::marked;

const KEEP_END: usize = 12;
const WHOLE_UP_TO: usize = 28;

pub const FILE_TAG_WIDTH: &str = "min-w-0 max-w-[min(55vw,18rem)]";

pub fn split_name(name: &str) -> (&str, &str) {
    let n = name.chars().count();
    if n <= WHOLE_UP_TO {
        return (name, "");
    }
    let at = name
        .char_indices()
        .nth(n - KEEP_END)
        .map_or(name.len(), |(i, _)| i);
    name.split_at(at)
}

#[component]
pub fn FileName(
    #[prop(into)] name: String,
    #[prop(optional)] class: &'static str,
    #[prop(optional, into)] mark: String,
    #[prop(optional)] lead: &'static str,
) -> impl IntoView {
    let (start, end) = split_name(&name);
    let (start, end) = (start.to_string(), end.to_string());
    let outer =
        format!("inline-block min-w-0 max-w-full truncate align-bottom tracking-normal {class}");
    if end.is_empty() {
        return view! { <span class=outer title=name.clone()>{lead}{marked(&name, &mark)}</span> }
            .into_any();
    }
    let room = format!(
        "max-width: round(down, calc(100% - {}ch), 1ch)",
        (lead.chars().count() + end.chars().count() + 1)
    );
    view! {
        <span class=outer title=name.clone()>
            {lead}
            <span class="inline-block truncate align-bottom" style=room data-name-start="">{marked(&start, &mark)}</span>
            {marked(&end, &mark)}
        </span>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_path_keeps_its_start_and_its_end() {
        assert_eq!(split_name("server.d/apps.yaml"), ("server.d/apps.yaml", ""));
        let long = format!("server.d/{}-end.yaml", "a".repeat(100));
        let (start, end) = split_name(&long);
        assert!(start.starts_with("server.d/aaa"));
        assert_eq!(end, "aaa-end.yaml", "the end stays whole");
        assert_eq!(format!("{start}{end}"), long);
        let wide = "server.d/ééééééééééééééééééééééééééééé-fin.yaml";
        let (s, e) = split_name(wide);
        assert_eq!(e.chars().count(), 12);
        assert_eq!(format!("{s}{e}"), wide, "splits on a character, not a byte");
    }
}
