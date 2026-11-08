use std::collections::BTreeMap;

use burrow_ui_kit::Segmented;
use config_editor_leptos::{DocumentsHandle, Language};
use leptos::prelude::*;

pub const LANGUAGES: [&str; 3] = ["YAML", "JSON", "TOML"];

pub fn language(label: &str) -> Option<Language> {
    match label {
        "YAML" => Some(Language::YAML),
        "JSON" => Some(Language::JSON),
        "TOML" => Some(Language::TOML),
        _ => None,
    }
}

pub fn own_label(name: &str) -> &'static str {
    Language::from_path(name).unwrap_or(Language::YAML).label()
}

pub fn choices(own: &str) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = LANGUAGES.iter().copied().filter(|l| *l == own).collect();
    out.extend(LANGUAGES.iter().copied().filter(|l| *l != own));
    out
}

pub fn requested(own: &str, asked: Option<&str>) -> Option<Language> {
    asked.filter(|a| *a != own).and_then(language)
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

#[derive(Clone, Copy)]
pub struct Switch {
    pub active: RwSignal<String>,
    pub asked: RwSignal<BTreeMap<String, String>>,
    pub shown: RwSignal<BTreeMap<String, String>>,
    pub tabs: DocumentsHandle,
}

impl Switch {
    pub fn refresh(&self, name: &str) {
        let own = own_label(name);
        let asked = self
            .asked
            .with_untracked(|a| a.get(name).cloned())
            .unwrap_or_else(|| own.to_string());
        let now = if asked == own {
            own.to_string()
        } else {
            let editor = self.tabs.editor(name);
            let on_screen = editor.and_then(|e| e.text());
            let host = editor.and_then(|e| e.value());
            shown(own, &asked, on_screen.as_deref(), host.as_deref())
        };
        if self.shown.with_untracked(|s| s.get(name) != Some(&now)) {
            self.shown.update(|s| {
                s.insert(name.to_string(), now);
            });
        }
    }

    pub fn refresh_soon(&self, name: String) {
        let me = *self;
        for ms in [60, 400, 1200] {
            let name = name.clone();
            set_timeout(
                move || me.refresh(&name),
                std::time::Duration::from_millis(ms),
            );
        }
    }

    pub fn view(self) -> impl IntoView {
        let options = Memo::new(move |_| {
            let own = self.active.with(|a| own_label(a));
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
            let name = self.active.get();
            self.asked
                .with(|a| a.get(&name).cloned())
                .unwrap_or_else(|| own_label(&name).to_string())
        });
        let note_text = move || {
            let name = self.active.get();
            let own = own_label(&name);
            let shown = self
                .shown
                .with(|s| s.get(&name).cloned())
                .unwrap_or_else(|| own.to_string());
            note(own, &shown)
        };
        let pick = Callback::new(move |label: String| {
            let name = self.active.get_untracked();
            self.asked.update(|a| {
                a.insert(name.clone(), label);
            });
            self.refresh_soon(name);
        });
        view! {
            <div class="flex min-w-0 flex-wrap items-center gap-2 text-xs text-text-muted" data-testid="file-edit-as" data-shown=move || { let n = self.active.get(); self.shown.with(|s| s.get(&n).cloned()).unwrap_or_else(|| own_label(&n).to_string()) }>
                <span aria-hidden="true">"Edit as"</span>
                {move || {
                    let options = options.get();
                    view! { <Segmented name="file-edit-as" label="Edit as" options=options value=value on_change=pick/> }
                }}
                {move || note_text().map(|n| view! { <span data-testid="file-saved-as">{n}</span> })}
            </div>
        }
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
        assert_eq!(own_label("server.json"), "JSON");
        assert_eq!(own_label("server.d/10-web.yml"), "YAML");
    }

    #[test]
    fn only_another_language_is_asked_of_the_editor() {
        assert!(requested("YAML", None).is_none());
        assert!(requested("YAML", Some("YAML")).is_none());
        assert_eq!(requested("YAML", Some("TOML")), Some(Language::TOML));
        assert_eq!(requested("JSON", Some("YAML")), Some(Language::YAML));
        assert!(requested("YAML", Some("XML")).is_none());
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
