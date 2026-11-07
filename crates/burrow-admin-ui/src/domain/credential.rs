use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::value::as_secret;
use burrow_syntax::env::{FILE_REF, Piece, pieces, valid_name};
use burrow_ui_kit::{Icon, Segmented, TextInput};
use leptos::prelude::*;

use crate::store::store;

use super::FieldCtx;

const VARIABLE: &str = "variable";
const FILE: &str = "file";
const TYPED: &str = "typed";

fn mode_of(v: Option<&Value>) -> &'static str {
    match v {
        Some(Value::String(s)) if s.trim().starts_with(FILE_REF) => FILE,
        Some(Value::String(s)) if matches!(pieces(s.trim()).as_slice(), [Piece::Var { .. }]) => {
            VARIABLE
        }
        Some(Value::String(s)) if s.is_empty() => VARIABLE,
        None => VARIABLE,
        _ => TYPED,
    }
}

fn inside(v: Option<&Value>, mode: &str) -> String {
    let Some(Value::String(s)) = v else {
        return String::new();
    };
    let s = s.trim();
    match mode {
        VARIABLE => s
            .strip_prefix("${")
            .and_then(|r| r.strip_suffix('}'))
            .unwrap_or_default()
            .to_string(),
        FILE => s
            .strip_prefix(FILE_REF)
            .unwrap_or_default()
            .trim()
            .to_string(),
        _ => s.to_string(),
    }
}

fn written(mode: &str, text: &str) -> Option<Value> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    Some(Value::String(match mode {
        VARIABLE => format!("${{{t}}}"),
        FILE => format!("{FILE_REF}{t}"),
        _ => text.to_string(),
    }))
}

fn guide(spath: &[String]) -> Option<String> {
    let provider = spath.iter().rev().nth(1)?;
    burrow_admin_logic::wording::dns_guide(provider)
}

pub(super) fn credential_editor(fc: FieldCtx) -> AnyView {
    let e = store().edit;
    let path = fc.path.clone();
    let now = Memo::new(move |_| e.value(&path));
    let picked: RwSignal<Option<&'static str>> = RwSignal::new(None);
    let mode = Memo::new(move |_| {
        picked
            .get()
            .unwrap_or_else(|| now.with(|v| mode_of(v.as_ref())))
    });
    let hidden = Memo::new(move |_| now.with(|v| v.as_ref().and_then(as_secret).is_some()));
    let options = vec![
        (VARIABLE.to_string(), "Variable".to_string(), false),
        (FILE.to_string(), "File".to_string(), false),
        (TYPED.to_string(), "Value".to_string(), false),
    ];
    let p2 = fc.path.clone();
    let on_mode = Callback::new(move |m: String| {
        let m: &'static str = match m.as_str() {
            FILE => FILE,
            TYPED => TYPED,
            _ => VARIABLE,
        };
        picked.set(Some(m));
        e.local_error(&p2, None);
        if !hidden.get_untracked() {
            let kept = inside(now.get_untracked().as_ref(), mode.get_untracked());
            e.set(&p2, written(m, &kept));
        }
    });
    let p3 = fc.path.clone();
    let touched = fc.touched;
    let on_input = Callback::new(move |t: String| {
        let m = mode.get_untracked();
        let bad = (m == VARIABLE && !t.trim().is_empty() && !valid_name(t.trim()))
            .then(|| "a variable name has letters, digits and _, like CF_TOKEN".to_string());
        e.local_error(&p3, bad);
        e.set(&p3, written(m, &t));
    });
    let value = Signal::derive(move || {
        if hidden.get() {
            String::new()
        } else {
            now.with(|v| inside(v.as_ref(), mode.get()))
        }
    });
    let invalid = {
        let path = fc.path.clone();
        Signal::derive(move || touched.get() && e.local.with(|l| l.contains_key(&path)))
    };
    let id = fc.id.clone();
    let msg = fc.msg_id.clone();
    let input = move || {
        let (prefix, suffix, placeholder, secret) = match mode.get() {
            VARIABLE => (
                Some("${".to_string()),
                Some("}".to_string()),
                "VARIABLE_NAME",
                false,
            ),
            FILE => (
                Some(FILE_REF.to_string()),
                None,
                "/etc/burrow/cf.token",
                false,
            ),
            _ => (
                None,
                None,
                if hidden.get() {
                    "set; type a new one to replace it"
                } else {
                    ""
                },
                true,
            ),
        };
        view! {
            <TextInput
                id=id.clone()
                value=value
                on_input=on_input
                on_blur=move |_| { let _ = touched.try_set(true); }
                placeholder=placeholder
                invalid=invalid
                described_by=msg.clone()
                mono=true
                prefix=prefix.unwrap_or_default()
                suffix=suffix.unwrap_or_default()
                new_password=secret
            />
        }
    };
    let link = guide(&fc.spath).map(|url| {
        view! {
            <a href=url target="_blank" rel="noopener noreferrer" class="inline-flex items-center gap-1 text-xs font-semibold text-accent-ink" data-testid="credential-guide">
                "How to make one"<Icon name="external" small=true/>
            </a>
        }
    });
    view! {
        <div class="flex flex-col gap-2" data-credential="">
            <Segmented name=format!("{}-source", fc.id) options=options value=Signal::derive(move || mode.get().to_string()) label=fc.node.label.clone() on_change=on_mode/>
            {input}
            {link}
        </div>
    }
    .into_any()
}
