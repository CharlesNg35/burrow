use burrow_ui_kit::{Icon, Select};
use leptos::prelude::*;

use crate::edit::TextEdit;

pub const LANGUAGES: [&str; 3] = ["YAML", "JSON", "TOML"];

pub fn own_label(name: &str) -> &'static str {
    if name.to_ascii_lowercase().ends_with(".json") {
        "JSON"
    } else {
        "YAML"
    }
}

pub fn choices(own: &str) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = LANGUAGES.iter().copied().filter(|l| *l == own).collect();
    out.extend(LANGUAGES.iter().copied().filter(|l| *l != own));
    out
}

#[derive(Clone, Debug, PartialEq)]
pub struct Seen {
    pub shown: String,
    pub asked_back: bool,
    pub settled: bool,
}

fn neutral(text: &str) -> bool {
    text.lines().all(|l| {
        let l = l.trim();
        l.is_empty() || l.starts_with('#')
    })
}

pub fn seen(
    own: &str,
    asked: &str,
    before: Option<&str>,
    picked: bool,
    on_screen: Option<&str>,
    host: Option<&str>,
) -> Seen {
    let lf = |t: &str| t.replace("\r\n", "\n");
    let other = |l: &str| (l != own).then(|| l.to_string());
    let shown = |l: String| Seen {
        settled: l == asked,
        shown: l,
        asked_back: false,
    };
    match (on_screen, host) {
        (None, _) => Seen {
            shown: asked.to_string(),
            asked_back: false,
            settled: false,
        },
        (Some(_), None) => shown(
            before
                .and_then(other)
                .or_else(|| other(asked))
                .unwrap_or_else(|| own.to_string()),
        ),
        (Some(screen), Some(_)) if neutral(screen) => shown(asked.to_string()),
        (Some(screen), Some(host)) if lf(screen) == lf(host) => {
            if picked && asked != own {
                shown(own.to_string())
            } else {
                Seen {
                    shown: own.to_string(),
                    asked_back: asked != own,
                    settled: true,
                }
            }
        }
        (Some(_), Some(_)) => shown(
            other(asked)
                .or_else(|| before.and_then(other))
                .unwrap_or_else(|| asked.to_string()),
        ),
    }
}

pub fn waits(asked: &str, shown: &str) -> Option<String> {
    (asked != shown).then(|| format!("{asked} waits for the marked value"))
}

fn shown_now(t: TextEdit) -> String {
    let name = t.active.get();
    t.shown
        .with(|s| s.get(&name).cloned())
        .unwrap_or_else(|| own_label(&name).to_string())
}

pub fn options(own: &str) -> Vec<(String, String)> {
    choices(own)
        .into_iter()
        .map(|l| {
            let text = if l == own {
                format!("{l} (file)")
            } else {
                l.to_string()
            };
            (l.to_string(), text)
        })
        .collect()
}

pub fn controls(t: TextEdit) -> impl IntoView {
    let options = Signal::derive(move || t.active.with(|a| options(own_label(a))));
    let pick = Callback::new(move |label: String| {
        let name = t.active.get_untracked();
        t.picked.update_value(|p| {
            p.insert(name.clone());
        });
        t.edit_as.update(|a| {
            a.insert(name, label);
        });
    });
    Effect::new(move |_| {
        let now = shown_now(t);
        if let Some(select) = document().get_element_by_id("file-edit-as") {
            let _ = js_sys::Reflect::set(&select, &"value".into(), &now.into());
        }
    });
    view! {
        <span class="flex min-w-0 items-center gap-2" data-testid="file-edit-as" data-shown=move || shown_now(t)>
            <span aria-hidden="true">"Edit as"</span>
            <Select id="file-edit-as" label="Edit as" options=options value=Signal::derive(move || shown_now(t)) on_change=pick small=true/>
        </span>
    }
}

pub fn read_only_text(t: TextEdit) -> Option<String> {
    t.loaded.with(|l| {
        let text = l.as_ref()?.as_ref().ok()?;
        t.active
            .with(|a| text.files.iter().find(|f| f.name == *a))
            .and_then(|f| Some(format!("{} is read-only: {}", f.name, f.read_only.clone()?)))
    })
}

pub fn waits_text(t: TextEdit) -> Option<String> {
    let name = t.active.get();
    let asked = t
        .edit_as
        .with(|a| a.get(&name).cloned())
        .unwrap_or_else(|| own_label(&name).to_string());
    waits(&asked, &shown_now(t))
}

pub fn note_line(icon: &'static str, text: String, testid: &'static str, warn: bool) -> AnyView {
    let tone = if warn {
        "text-warning"
    } else {
        "text-text-muted"
    };
    view! {
        <p class=format!("flex min-w-0 items-start gap-1.5 text-xs {tone} [overflow-wrap:anywhere]") data-testid=testid>
            <Icon name=icon small=true/>
            <span>{text}</span>
        </p>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_language_comes_first() {
        assert_eq!(choices("YAML"), ["YAML", "JSON", "TOML"]);
        assert_eq!(choices("JSON"), ["JSON", "YAML", "TOML"]);
        assert_eq!(own_label("server.yaml"), "YAML");
        assert_eq!(own_label("server.JSON"), "JSON");
        assert_eq!(own_label("server.d/10-web.yml"), "YAML");
    }

    #[test]
    fn the_label_follows_what_is_on_screen() {
        let host = "a: 1\nb: null\n";
        let at =
            |asked: &str,
             before: Option<&str>,
             picked: bool,
             screen: Option<&str>,
             h: Option<&str>| { seen("YAML", asked, before, picked, screen, h) };
        assert_eq!(
            at("YAML", None, false, Some(host), Some(host)).shown,
            "YAML"
        );
        assert_eq!(
            at("TOML", None, true, Some("a = 1\n"), Some(host)).shown,
            "TOML"
        );
        let waits_now = at("TOML", None, true, Some(host), Some(host));
        assert_eq!(
            (waits_now.shown.as_str(), waits_now.asked_back),
            ("YAML", false),
            "a switch just picked that waits still shows the file's own language"
        );
        let undone = at("TOML", Some("TOML"), false, Some(host), Some(host));
        assert_eq!(
            (undone.shown.as_str(), undone.asked_back, undone.settled),
            ("YAML", true, true),
            "an Undo back to the file's text: the select goes back to the file's language"
        );
        assert_eq!(
            at("YAML", Some("TOML"), false, Some("broken = [\n"), None).shown,
            "TOML",
            "asking for the file's language back while the TOML cannot convert: TOML stays on screen"
        );
        assert_eq!(
            at("JSON", None, true, Some("{\"a\": 1"), None).shown,
            "JSON"
        );
        let empty = at("TOML", None, true, Some(""), Some(""));
        assert_eq!(
            (empty.shown.as_str(), empty.settled),
            ("TOML", true),
            "an empty or comment-only file reads as the language asked"
        );
        assert_eq!(
            at(
                "TOML",
                None,
                true,
                Some("# only a comment\n"),
                Some("# only a comment\n")
            )
            .shown,
            "TOML"
        );
        assert_eq!(
            at("TOML", Some("TOML"), false, None, None).shown,
            "TOML",
            "not mounted: as asked"
        );
        assert_eq!(
            waits("YAML", "TOML").as_deref(),
            Some("YAML waits for the marked value")
        );
        assert_eq!(waits("TOML", "TOML"), None, "no note once it shows");
        assert_eq!(waits("YAML", "YAML"), None);
        assert_eq!(
            waits("TOML", "YAML").as_deref(),
            Some("TOML waits for the marked value")
        );
        assert_eq!(
            options("JSON"),
            [
                ("JSON".to_string(), "JSON (file)".to_string()),
                ("YAML".to_string(), "YAML".to_string()),
                ("TOML".to_string(), "TOML".to_string())
            ]
        );
    }
}
