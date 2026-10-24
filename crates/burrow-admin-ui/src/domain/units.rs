use burrow_admin_logic::input::{rate_parts, rate_value};
use burrow_admin_proto::serde_json::Value;
use burrow_syntax::duration::{TimeUnit, best_unit, human, to_stored, units_for};
use burrow_syntax::rate::RateUnit;
use burrow_ui_kit::{Segmented, Select, TextInput};
use leptos::prelude::*;

use crate::store::store;

use super::{FieldCtx, placeholder, set_checked};

pub(super) fn duration_editor(fc: FieldCtx, stored: TimeUnit, max: Option<u64>) -> AnyView {
    let e = store().edit;
    let units = units_for(stored, max);
    let path = fc.path.clone();
    let start = e.peek(&path).and_then(|v| v.as_u64());
    let unit = RwSignal::new(
        start
            .map(|v| best_unit(v, stored, &units))
            .unwrap_or(stored),
    );
    let number = RwSignal::new(
        start
            .map(|v| (v.saturating_mul(stored.ms()) / unit.get_untracked().ms()).to_string())
            .unwrap_or_default(),
    );
    let node = fc.node.clone();
    let fc2 = fc.clone();
    let commit = move || {
        let n = number.get_untracked();
        if n.trim().is_empty() {
            set_checked(&fc2, &node, None);
            return;
        }
        match to_stored(&n, unit.get_untracked(), stored) {
            Ok(v) => set_checked(&fc2, &node, Some(Value::from(v))),
            Err(msg) => {
                e.local_error(&fc2.path, Some(msg));
                e.set(&fc2.path, Some(Value::String(n)));
            }
        }
    };
    let c2 = commit.clone();
    let options: Vec<(String, String)> = units
        .iter()
        .map(|u| (u.short().to_string(), u.label().to_string()))
        .collect();
    let units2 = units.clone();
    let p2 = fc.path.clone();
    let shown = move || {
        e.value(&p2)
            .and_then(|v| v.as_u64())
            .map(|v| format!("= {}", human(v, stored)))
    };
    let touched = fc.touched;
    let invalid = {
        let path = fc.path.clone();
        Signal::derive(move || touched.get() && e.local.with(|l| l.contains_key(&path)))
    };
    view! {
        <div class="flex flex-wrap items-center gap-2">
            <div class="w-[120px]">
                <TextInput
                    id=fc.id.clone()
                    value=number
                    numeric=true
                    invalid=invalid
                    described_by=fc.msg_id.clone()
                    placeholder=placeholder(&fc)
                    on_input=move |t: String| {
                        number.set(t);
                        commit();
                    }
                    on_blur=move |_| { let _ = touched.try_set(true); }
                />
            </div>
            <div class="w-28">
                <Select
                    id=format!("{}-unit", fc.id)
                    label=format!("{} unit", fc.node.label)
                    options=options
                    value=Signal::derive(move || unit.get().short().to_string())
                    on_change=move |u: String| {
                        if let Some(x) = units2.iter().find(|x| x.short() == u) {
                            unit.set(*x);
                            c2();
                        }
                    }
                />
            </div>
            <span class="text-sm text-text-muted">{shown}</span>
        </div>
    }
    .into_any()
}

pub(super) fn rate_editor(fc: FieldCtx) -> AnyView {
    let e = store().edit;
    let start = e.peek(&fc.path);
    let limited = RwSignal::new(start.as_ref().is_some_and(|v| !v.is_null()));
    let (n0, u0) = rate_parts(start.as_ref());
    let number = RwSignal::new(n0);
    let unit = RwSignal::new(u0);
    let node = fc.node.clone();
    let fc2 = fc.clone();
    let commit = move || {
        set_checked(
            &fc2,
            &node,
            rate_value(&number.get_untracked(), unit.get_untracked()),
        )
    };
    let c2 = commit.clone();
    let c3 = commit.clone();
    let options: Vec<(String, String)> = RateUnit::ALL
        .iter()
        .map(|u| (u.suffix().to_string(), u.label().to_string()))
        .collect();
    let p2 = fc.path.clone();
    let equivalent = move || {
        e.value(&p2)
            .and_then(|v| v.as_str().map(str::to_string))
            .and_then(|t| {
                burrow_syntax::rate::parse_rate(&t)
                    .ok()
                    .map(|b| format!("≈ {} · stored as {t}", burrow_syntax::rate::equivalent(b)))
            })
            .unwrap_or_default()
    };
    let touched = fc.touched;
    let invalid = {
        let path = fc.path.clone();
        Signal::derive(move || touched.get() && e.local.with(|l| l.contains_key(&path)))
    };
    let fc3 = fc.clone();
    let node3 = fc.node.clone();
    let id = fc.id.clone();
    let msg_id = fc.msg_id.clone();
    let label = fc.node.label.clone();
    view! {
        <div class="flex flex-wrap items-center gap-2">
            <Segmented
                name=format!("{}-mode", fc.id)
                label=format!("{} mode", fc.node.label)
                options=vec![("unlimited".to_string(), "Unlimited".to_string(), false), ("limit".to_string(), "Limit".to_string(), false)]
                value=Signal::derive(move || if limited.get() { "limit".to_string() } else { "unlimited".to_string() })
                on_change=Callback::new(move |v: String| {
                    let on = v == "limit";
                    limited.set(on);
                    if on {
                        if number.get_untracked().trim().is_empty() {
                            number.set("10".into());
                        }
                        c3();
                    } else {
                        set_checked(&fc3, &node3, None);
                    }
                })
            />
            <Show when=move || limited.get()>
                <div class="w-[100px]">
                    <TextInput
                        id=id.clone()
                        value=number
                        numeric=true
                        invalid=invalid
                        described_by=msg_id.clone()
                        on_input={
                            let commit = commit.clone();
                            move |t: String| {
                                number.set(t);
                                commit();
                            }
                        }
                        on_blur=move |_| { let _ = touched.try_set(true); }
                    />
                </div>
                <div class="w-32">
                    <Select
                        id=format!("{}-unit", id)
                        label=format!("{label} unit")
                        options=options.clone()
                        value=Signal::derive(move || unit.get().suffix().to_string())
                        on_change={
                            let c2 = c2.clone();
                            move |u: String| {
                                if let Some(x) = RateUnit::from_suffix(&u) {
                                    unit.set(x);
                                    c2();
                                }
                            }
                        }
                    />
                </div>
                <span class="font-mono text-xs text-text-muted">{equivalent.clone()}</span>
            </Show>
        </div>
    }
    .into_any()
}
