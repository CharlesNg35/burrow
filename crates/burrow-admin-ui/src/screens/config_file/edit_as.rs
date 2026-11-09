use burrow_ui_kit::{Icon, Select};
use leptos::prelude::*;

use crate::edit::TextEdit;

pub const LANGUAGES: [&str; 3] = ["YAML", "JSON", "TOML"];

pub use crate::edit::own_label;

pub fn choices(own: &str) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = LANGUAGES.iter().copied().filter(|l| *l == own).collect();
    out.extend(LANGUAGES.iter().copied().filter(|l| *l != own));
    out
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
        let same = t.edit_as.with_untracked(|a| a.get(&name) == Some(&label));
        if same {
            t.ask_again(&name, &label);
        } else {
            t.edit_as.update(|a| {
                a.insert(name, label);
            });
        }
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
