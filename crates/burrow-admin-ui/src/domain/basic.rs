use burrow_admin_logic::fallback::{Render, for_node};
use burrow_admin_logic::input::{parse_input, to_text};
use burrow_admin_proto::schema::{EnumC, SemType};
use burrow_admin_proto::serde_json::{self, Value};
use burrow_ui_kit::{Segmented, Select, TextArea, TextInput, Toggle};
use leptos::prelude::*;

use crate::grants::in_the_file;
use crate::store::store;

use super::{FieldCtx, placeholder, set_checked};

pub(super) fn text_editor(fc: FieldCtx, sem: Option<SemType>) -> AnyView {
    let e = store().edit;
    let path = fc.path.clone();
    let value = Signal::derive(move || to_text(e.value(&path).as_ref()));
    let node = fc.node.clone();
    let numeric = matches!(
        sem,
        Some(SemType::Integer(_) | SemType::Size(_) | SemType::Port)
    );
    let mono = !matches!(sem, Some(SemType::Text(_)) | None);
    let invalid = {
        let path = fc.path.clone();
        let touched = fc.touched;
        Signal::derive(move || touched.get() && e.local.with(|l| l.contains_key(&path)))
    };
    let fc2 = fc.clone();
    let on_input = move |text: String| {
        let v = match &sem {
            Some(s) => parse_input(s, &text),
            None => (!text.trim().is_empty()).then(|| Value::String(text.trim().to_string())),
        };
        if sem.is_some() {
            set_checked(&fc2, &node, v);
        } else {
            e.local_error(&fc2.path, None);
            e.set(&fc2.path, v);
        }
    };
    let touched = fc.touched;
    let unit = match &fc.node.sem {
        SemType::Integer(c) => c.unit.clone(),
        SemType::Size(c) => Some(c.unit.clone()),
        SemType::Optional(o) => match &o.inner.sem {
            SemType::Integer(c) => c.unit.clone(),
            SemType::Size(c) => Some(c.unit.clone()),
            _ => None,
        },
        _ => None,
    };
    let server_checked = matches!(
        for_node(&fc.node, None),
        Render::ServerChecked | Render::Fallback(_)
    );
    let width = if numeric {
        "w-[240px] max-w-full"
    } else {
        "min-w-0 flex-1"
    };
    view! {
        <div class="flex items-center gap-2">
            <div class=width>
                <TextInput
                    id=fc.id.clone()
                    value=value
                    on_input=on_input
                    on_blur=move |_| { let _ = touched.try_set(true); }
                    placeholder=placeholder(&fc)
                    invalid=invalid
                    described_by=fc.msg_id.clone()
                    numeric=numeric
                    mono=mono
                    suffix=unit.unwrap_or_default()
                />
            </div>
            {server_checked.then(|| view! { <span data-render="server-checked">{crate::form::server_checked()}</span> })}
        </div>
    }
    .into_any()
}

pub(super) fn yaml_editor(fc: FieldCtx) -> AnyView {
    let e = store().edit;
    let path = fc.path.clone();
    let initial = e
        .peek(&path)
        .map(|v| serde_json::to_string_pretty(&v).unwrap_or_default())
        .unwrap_or_default();
    let p2 = path.clone();
    let value = Signal::derive(move || {
        e.texts
            .with(|t| t.get(&p2).cloned())
            .unwrap_or_else(|| initial.clone())
    });
    view! {
        <div class="rounded-card border border-dashed border-warning p-2" data-testid="raw-node">
            <p class="mb-1 text-sm text-warning">"This setting needs a newer burrow admin to edit it in a form. Edit it as YAML; the server checks it."</p>
            <TextArea id=fc.id.clone() value=value on_input=move |t: String| e.set_text(&path, (!t.trim().is_empty()).then_some(t)) rows=4/>
        </div>
    }
    .into_any()
}

const INHERIT: &str = "inherit";

fn tri_options(none_label: &str) -> Vec<(String, String, bool)> {
    let mut out = vec![(INHERIT.to_string(), none_label.to_string(), false)];
    for (v, l) in [("true", "Yes"), ("false", "No")] {
        if l != none_label {
            out.push((v.to_string(), l.to_string(), false));
        }
    }
    out
}

fn tri_value(none_label: &str, v: Option<&Value>) -> String {
    match v.and_then(Value::as_bool) {
        Some(true) if none_label != "Yes" => "true".into(),
        Some(false) if none_label != "No" => "false".into(),
        _ => INHERIT.into(),
    }
}

fn tri_editor(fc: FieldCtx, none_label: String) -> AnyView {
    let e = store().edit;
    let path = fc.path.clone();
    let label = none_label.clone();
    let value = Signal::derive(move || tri_value(&label, e.value(&path).as_ref()));
    let p2 = fc.path.clone();
    view! {
        <div id=fc.id.clone()>
            <Segmented
                name=format!("{}-choice", fc.id)
                options=tri_options(&none_label)
                value=value
                label=fc.node.label.clone()
                on_change=move |m: String| e.set(&p2, m.parse::<bool>().ok().map(Value::Bool))
            />
        </div>
    }
    .into_any()
}

pub(super) fn bool_editor(fc: FieldCtx) -> AnyView {
    if let Some(none) = fc.node.none_label.clone().filter(|_| !fc.node.required) {
        return tri_editor(fc, none);
    }
    let e = store().edit;
    let path = fc.path.clone();
    let default = fc
        .node
        .default
        .as_ref()
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let checked =
        Signal::derive(move || e.value(&path).and_then(|v| v.as_bool()).unwrap_or(default));
    let p2 = fc.path.clone();
    let danger = fc.node.danger_when_set;
    view! {
        <Toggle
            id=fc.id.clone()
            checked=checked
            label=Signal::derive(move || if checked.get() { "Yes".to_string() } else { "No".to_string() })
            danger=danger
            on_change=move |on: bool| {
                let v = if on == default && !danger { None } else { Some(Value::Bool(on)) };
                e.set(&p2, v)
            }
        />
    }
    .into_any()
}

const UNSET: &str = "\u{1}unset";

pub(super) fn enum_editor(fc: FieldCtx, c: EnumC) -> AnyView {
    let e = store().edit;
    let path = fc.path.clone();
    if fc.node.reduce_only
        && e.draft
            .with_untracked(|d| burrow_admin_proto::value::get(d.base(), &path).is_none())
    {
        let none = fc.node.none_label.clone().unwrap_or_else(|| "None".into());
        return view! {
            <div id=fc.id.clone() class="flex min-h-[38px] flex-col justify-center text-[13px]" data-testid="grant-none">
                <span>{none}</span>
                {crate::grants::in_the_file()}
            </div>
        }
        .into_any();
    }
    let optional = !fc.node.required;
    let default_label = fc
        .node
        .default
        .as_ref()
        .map(|d| {
            let raw = to_text(Some(d));
            let label = c
                .options
                .iter()
                .find(|o| o.value == raw)
                .map(|o| o.label.clone())
                .unwrap_or(raw);
            format!("Default ({label})")
        })
        .unwrap_or_else(|| {
            fc.node
                .none_label
                .clone()
                .unwrap_or_else(|| "Not set".into())
        });
    let default_raw = fc.node.default.as_ref().map(|d| to_text(Some(d)));
    let merge_default = optional
        && default_raw
            .as_ref()
            .is_some_and(|d| c.options.iter().any(|o| &o.value == d));
    let shown_default = default_raw.clone();
    let value = Signal::derive(move || {
        e.value(&path)
            .map(|v| to_text(Some(&v)))
            .unwrap_or_else(|| {
                if merge_default {
                    shown_default.clone().unwrap_or_default()
                } else {
                    UNSET.to_string()
                }
            })
    });
    let p2 = fc.path.clone();
    let set = move |v: String| {
        e.local_error(&p2, None);
        let clear = v == UNSET
            || (merge_default
                && Some(&v) == default_raw.as_ref()
                && e.draft
                    .with_untracked(|d| burrow_admin_proto::value::get(d.base(), &p2).is_none()));
        e.set(&p2, (!clear).then(|| Value::String(v)));
    };
    let mut options: Vec<(String, String, bool)> = Vec::new();
    if optional && !merge_default {
        options.push((UNSET.to_string(), default_label, false));
    }
    options.extend(
        c.options
            .iter()
            .map(|o| (o.value.clone(), o.label.clone(), o.danger)),
    );
    let gated = fc.path.clone();
    let values: Vec<String> = options.iter().map(|o| o.0.clone()).collect();
    let off = Signal::derive(move || {
        values
            .iter()
            .filter(|v| {
                let candidate = (*v != UNSET).then(|| Value::String(v.to_string()));
                crate::grants::refuses(&gated, candidate.as_ref())
            })
            .cloned()
            .collect::<Vec<_>>()
    });
    let set = move |v: String| {
        if !off.with_untracked(|o| o.contains(&v)) {
            set(v);
        }
    };
    let locked = move || off.with(|o| !o.is_empty()).then(in_the_file);
    let help = {
        let c = c.clone();
        let field_help = !fc.node.help.is_empty();
        move || {
            if field_help {
                return None;
            }
            let v = value.get();
            c.options
                .iter()
                .find(|o| o.value == v)
                .map(|o| o.help.clone())
                .filter(|h| !h.is_empty())
                .map(|h| view! { <p class="mt-1 text-sm text-text-muted">{h}</p> })
        }
    };
    let control = choice(
        fc.id.clone(),
        fc.node.label.clone(),
        options,
        value,
        Callback::new(set),
        off,
    );
    view! { <div>{control}{locked}{help}</div> }.into_any()
}

const SEGMENTED_MAX: usize = 4;

pub(crate) fn choice(
    id: String,
    label: String,
    options: Vec<(String, String, bool)>,
    value: Signal<String>,
    on_change: Callback<String>,
    off: Signal<Vec<String>>,
) -> AnyView {
    if options.len() <= SEGMENTED_MAX {
        view! { <div id=id.clone()><Segmented name=id.clone() options=options value=value on_change=on_change label=label off=off/></div> }.into_any()
    } else {
        let opts: Vec<(String, String)> = options.into_iter().map(|(v, l, _)| (v, l)).collect();
        view! { <Select id=id options=opts value=value on_change=on_change/> }.into_any()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::serde_json::json;

    #[test]
    fn an_optional_yes_or_no_offers_its_unset_meaning_first_without_repeating_it() {
        let labels = |n: &str| {
            tri_options(n)
                .into_iter()
                .map(|(v, l, _)| format!("{v}={l}"))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            labels("As the web proxy says"),
            ["inherit=As the web proxy says", "true=Yes", "false=No"]
        );
        assert_eq!(labels("Yes"), ["inherit=Yes", "false=No"]);
        assert_eq!(tri_value("Yes", Some(&json!(true))), "inherit");
        assert_eq!(tri_value("Yes", Some(&json!(false))), "false");
        assert_eq!(
            tri_value("As the web proxy says", Some(&json!(true))),
            "true"
        );
        assert_eq!(tri_value("As the web proxy says", None), "inherit");
    }
}
