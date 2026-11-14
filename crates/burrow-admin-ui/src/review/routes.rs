use burrow_ui_kit::{HopState, Icon, PathData, PathDiagram, Tag, Tone};
use leptos::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    New,
    Changed,
    Removed,
}

fn kind_of(p: &PathData) -> Kind {
    let all = |s: HopState| !p.hops.is_empty() && p.hops.iter().all(|h| h.state == s);
    if all(HopState::Removed) {
        Kind::Removed
    } else if all(HopState::Added) {
        Kind::New
    } else {
        Kind::Changed
    }
}

fn route(p: &PathData) -> String {
    p.hops
        .iter()
        .skip(1)
        .map(|h| h.value.as_str())
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>()
        .join(" → ")
}

fn line_of(p: &PathData) -> String {
    if kind_of(p) != Kind::Changed {
        return route(p);
    }
    let parts: Vec<String> = p
        .hops
        .iter()
        .filter(|h| h.state == HopState::Added)
        .map(|h| {
            let head = h.head.to_lowercase();
            match h.badge.as_ref().and_then(|(b, _)| b.strip_prefix("was ")) {
                Some(was) => format!("{head} {was} → {}", h.value),
                None => format!("{head} {}", h.value),
            }
        })
        .collect();
    if parts.is_empty() {
        "other settings".into()
    } else {
        parts.join(" · ")
    }
}

fn tag_of(k: Kind) -> (&'static str, Tone) {
    match k {
        Kind::New => ("new", Tone::Success),
        Kind::Changed => ("changed", Tone::Info),
        Kind::Removed => ("removed", Tone::Neutral),
    }
}

pub(super) fn diagrams(mut paths: Vec<(String, PathData)>) -> AnyView {
    if paths.len() == 1 {
        let (name, p) = paths.remove(0);
        return view! { <div data-review-path=name><PathDiagram data=p/></div> }.into_any();
    }
    let rows = paths
        .into_iter()
        .map(|(name, p)| {
            let line = line_of(&p);
            let title = line.clone();
            let key = name.clone();
            let (tag, tone) = tag_of(kind_of(&p));
            view! {
                <details class="group" data-review-path=key>
                    <summary class="flex cursor-pointer list-none items-center gap-3 rounded-card border border-border bg-surface px-4 py-2.5 text-[13px] hover:bg-surface-2 group-open:mb-2 [&::-webkit-details-marker]:hidden" data-testid="review-path-line">
                        <span class="flex shrink-0 text-text-muted transition-transform group-open:rotate-90"><Icon name="chevron-right" small=true/></span>
                        <span class="shrink-0 font-semibold">{name}</span>
                        <span class="min-w-0 flex-1 truncate font-mono text-xs text-text-muted" title=title>{line}</span>
                        <Tag text=tag.to_string() tone=tone/>
                    </summary>
                    <PathDiagram data=p/>
                </details>
            }
        })
        .collect_view();
    view! { <div class="mb-[22px] flex flex-col gap-2" data-testid="review-paths">{rows}</div> }
        .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_ui_kit::Hop;

    fn hop(head: &str, value: &str, state: HopState, was: Option<&str>) -> Hop {
        Hop {
            head: head.into(),
            value: value.into(),
            state,
            badge: was.map(|w| (format!("was {w}"), Tone::Neutral)),
            ..Hop::default()
        }
    }

    fn path(hops: Vec<Hop>) -> PathData {
        PathData {
            hops,
            ..PathData::default()
        }
    }

    #[test]
    fn a_changed_service_names_what_changed_from_and_to() {
        use HopState::{Added, Normal};
        let p = path(vec![
            hop("Internet", "anyone", Normal, None),
            hop("Server", "127.0.0.1:23728", Normal, None),
            hop("Client", "home", Normal, None),
            hop("Target", "127.0.0.1:2222", Added, Some("127.0.0.1:22")),
        ]);
        assert_eq!(kind_of(&p), Kind::Changed);
        assert_eq!(line_of(&p), "target 127.0.0.1:22 → 127.0.0.1:2222");
    }

    #[test]
    fn a_new_or_removed_service_shows_its_route() {
        let hops = |s| {
            vec![
                hop("Internet", "anyone", s, None),
                hop("Server", "0.0.0.0:8080", s, None),
                hop("Client", "home", s, None),
                hop("Target", "127.0.0.1:3000", s, None),
            ]
        };
        let new = path(hops(HopState::Added));
        assert_eq!(kind_of(&new), Kind::New);
        assert_eq!(line_of(&new), "0.0.0.0:8080 → home → 127.0.0.1:3000");
        let gone = path(hops(HopState::Removed));
        assert_eq!(kind_of(&gone), Kind::Removed);
        assert_eq!(line_of(&gone), "0.0.0.0:8080 → home → 127.0.0.1:3000");
    }

    #[test]
    fn a_change_the_diagram_does_not_show_is_other_settings() {
        let p = path(vec![
            hop("Internet", "anyone", HopState::Normal, None),
            hop("Server", "0.0.0.0:8080", HopState::Normal, None),
        ]);
        assert_eq!(line_of(&p), "other settings");
    }
}
