//! The editor's colors: the read-only view's (`burrow_ui_kit::CodeBlock`), so Edit looks the same.
//! The code surface is dark in both of burrow admin's themes, so the editor's chrome is too.

use leptos_config_editor::{ThemeColor as C, ThemeColors};

const KEY: &str = "#8cc4f5";
const VALUE: &str = "#b5e3a8";
const ACCENT: &str = "#7dbdf5";
const CODE: &str = "var(--color-code)";
const TEXT: &str = "var(--color-code-text)";
const FAINT: &str = "color-mix(in srgb, var(--color-code-text) 45%, transparent)";
const MUTED: &str = "color-mix(in srgb, var(--color-code-text) 62%, transparent)";
const PANEL: &str = "color-mix(in srgb, var(--color-code) 94%, white)";
const RAISED: &str = "color-mix(in srgb, var(--color-code) 88%, white)";
const LINE: &str = "rgb(255 255 255 / 10%)";

pub fn colors() -> ThemeColors {
    [
        (C::Background, CODE),
        (C::Text, TEXT),
        (C::Muted, MUTED),
        (C::Key, KEY),
        (C::Value, VALUE),
        (C::String, VALUE),
        (C::Literal, VALUE),
        (C::Punctuation, TEXT),
        (C::Comment, FAINT),
        (C::Caret, TEXT),
        (C::Accent, ACCENT),
        (C::FocusRing, ACCENT),
        (C::Selection, "rgb(125 189 245 / 26%)"),
        (C::ActiveLine, "rgb(255 255 255 / 4%)"),
        (C::LineNumber, FAINT),
        (C::LineNumberActive, TEXT),
        (C::Error, "#ff6369"),
        (C::Warning, "#f5b547"),
        (C::Info, ACCENT),
        (C::Added, "#3dd68c"),
        (C::Modified, ACCENT),
        (C::Panel, PANEL),
        (C::Border, LINE),
        (C::ToolbarText, TEXT),
        (C::ControlBackground, "rgb(255 255 255 / 5%)"),
        (C::ControlText, TEXT),
        (C::InputBackground, CODE),
        (C::InputBorder, LINE),
        (C::Raised, RAISED),
        (C::RaisedBorder, "rgb(255 255 255 / 14%)"),
        (C::RaisedText, TEXT),
        (C::RaisedMuted, MUTED),
        (C::RaisedHover, "rgb(255 255 255 / 6%)"),
        (C::CompletionActive, "rgb(125 189 245 / 18%)"),
        (C::TabBar, PANEL),
        (C::TabBackground, PANEL),
        (C::TabActiveBackground, CODE),
        (C::TabText, MUTED),
        (C::TabActiveText, TEXT),
        (C::TabBorder, LINE),
        (C::TabActiveBorder, ACCENT),
    ]
    .into_iter()
    .fold(ThemeColors::new(), |c, (k, v)| c.set(k, v))
}
