use burrow_ui_kit::{Icon, Segmented};
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

pub fn shown(own: &str, asked: &str, on_screen: Option<&str>, host: Option<&str>) -> String {
    if asked == own {
        return own.to_string();
    }
    let lf = |t: &str| t.replace("\r\n", "\n");
    match (on_screen, host) {
        (Some(screen), Some(host)) if lf(screen) == lf(host) => own.to_string(),
        _ => asked.to_string(),
    }
}

pub fn note(own: &str, shown: &str) -> Option<String> {
    (own != shown).then(|| format!("Saved as {own}"))
}

fn shown_now(t: TextEdit) -> String {
    let name = t.active.get();
    t.shown
        .with(|s| s.get(&name).cloned())
        .unwrap_or_else(|| own_label(&name).to_string())
}

pub fn controls(t: TextEdit) -> impl IntoView {
    let options = Memo::new(move |_| {
        let own = t.active.with(|a| own_label(a));
        choices(own)
            .into_iter()
            .map(|l| {
                let text = if l == own {
                    format!("{l} (file)")
                } else {
                    l.to_string()
                };
                (l.to_string(), text, false)
            })
            .collect::<Vec<_>>()
    });
    let value = Signal::derive(move || {
        let name = t.active.get();
        t.edit_as
            .with(|a| a.get(&name).cloned())
            .unwrap_or_else(|| own_label(&name).to_string())
    });
    let pick = Callback::new(move |label: String| {
        let name = t.active.get_untracked();
        t.edit_as.update(|a| {
            a.insert(name, label);
        });
    });
    let saved_as = move || {
        let own = t.active.with(|a| own_label(a));
        note(own, &shown_now(t))
    };
    view! {
        <span class="flex min-w-0 flex-wrap items-center gap-2" data-testid="file-edit-as" data-shown=move || shown_now(t)>
            {move || {
                let options = options.get();
                view! { <Segmented name="file-edit-as" label="Edit as" options=options value=value on_change=pick/> }
            }}
            {move || saved_as().map(|n| view! { <span data-testid="file-saved-as">{n}</span> })}
        </span>
    }
}

pub fn read_only_note(t: TextEdit) -> impl IntoView {
    move || {
        let (name, why) = t.loaded.with(|l| {
            let text = l.as_ref()?.as_ref().ok()?;
            t.active
                .with(|a| text.files.iter().find(|f| f.name == *a))
                .and_then(|f| Some((f.name.clone(), f.read_only.clone()?)))
        })?;
        Some(view! {
            <span class="flex min-w-0 items-start gap-1.5 [overflow-wrap:anywhere]" data-testid="file-read-only">
                <Icon name="lock" small=true/>
                <span>{format!("{name} is read-only: {why}")}</span>
            </span>
        })
    }
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
        assert_eq!(shown("YAML", "YAML", Some(host), Some(host)), "YAML");
        assert_eq!(shown("YAML", "TOML", Some("a = 1\n"), Some(host)), "TOML");
        assert_eq!(
            shown("YAML", "TOML", Some(host), Some(host)),
            "YAML",
            "a switch that waits still shows the file's own language"
        );
        assert_eq!(
            shown("YAML", "JSON", Some("{\"a\": 1}"), None),
            "JSON",
            "an edit not yet written back is still the asked language"
        );
        assert_eq!(note("YAML", "TOML").as_deref(), Some("Saved as YAML"));
        assert_eq!(note("YAML", "YAML"), None);
    }
}
