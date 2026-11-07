use leptos::prelude::*;

use crate::basic::Tone;
use crate::field::tag_class;

#[component]
pub fn VersionTag(
    #[prop(into)] text: String,
    #[prop(optional)] muted: bool,
    #[prop(default = None)] tag: Option<(String, Tone)>,
    #[prop(optional, into)] why: String,
) -> impl IntoView {
    let class = if muted {
        "text-text-muted"
    } else {
        "font-mono text-[13px]"
    };
    view! {
        <span class=class data-testid="client-version">{text}</span>
        {tag.map(|(t, tone)| view! { <span class=format!("ml-1.5 whitespace-nowrap {}", tag_class(tone)) title=(!why.is_empty()).then_some(why) data-testid="client-update-tag">{t}</span> })}
    }
}
