use burrow_admin_proto::serde_json::{Value, json};
use burrow_config::{Entry, ServiceEntry, TargetCa, TargetUnchecked, WebSite};
use burrow_ui_kit::Segmented;
use leptos::prelude::*;

use crate::store::store;

use super::FieldCtx;

const OFF: &str = "off";
const PUBLIC: &str = "public";
const CA: &str = "own-ca";
const UNCHECKED: &str = "unchecked";
const CA_IN_THE_FILE: &str =
    "Checked by my CA is chosen in server.yaml (burrow edit), where the CA file is named";

pub(super) fn is_target_tls(fc: &FieldCtx) -> bool {
    matches!(fc.spath.as_slice(), [s, _, w, t] if s == ServiceEntry::SECTION && w == ServiceEntry::WEB.key() && t == WebSite::TARGET_TLS.key())
}

fn mode_of(v: Option<&Value>) -> &'static str {
    match v {
        Some(Value::Bool(true)) => PUBLIC,
        Some(Value::Object(m)) if m.contains_key(TargetCa::CA.key()) => CA,
        Some(Value::Object(m))
            if m.get(TargetUnchecked::INSECURE_SKIP_VERIFY.key()) == Some(&Value::Bool(true)) =>
        {
            UNCHECKED
        }
        Some(Value::Object(_)) => PUBLIC,
        _ => OFF,
    }
}

pub(super) fn target_tls_editor(fc: FieldCtx) -> AnyView {
    let e = store().edit;
    let path = fc.path.clone();
    let value = Memo::new(move |_| e.value(&path));
    let mode = Signal::derive(move || mode_of(value.get().as_ref()).to_string());
    let options = vec![
        (
            OFF.to_string(),
            "Off: plain HTTP to the app".to_string(),
            false,
        ),
        (
            PUBLIC.to_string(),
            "Checked by public CAs".to_string(),
            false,
        ),
        (CA.to_string(), "Checked by my CA (file)".to_string(), false),
        (UNCHECKED.to_string(), "Not checked".to_string(), false),
    ];
    let off = Signal::derive(move || {
        if mode.get() == CA {
            Vec::new()
        } else {
            vec![CA.to_string()]
        }
    });
    let p2 = fc.path.clone();
    let touched = fc.touched;
    let on_mode = move |m: String| {
        let v = match m.as_str() {
            PUBLIC => Some(Value::Bool(true)),
            UNCHECKED => Some(json!({ TargetUnchecked::INSECURE_SKIP_VERIFY.key(): true })),
            CA => return,
            _ => None,
        };
        touched.set(true);
        e.set(&p2, v);
    };
    let shown = move || {
        let v = value.get();
        let ca = v
            .as_ref()
            .and_then(|v| v.get(TargetCa::CA.key()))
            .and_then(Value::as_str)
            .map(str::to_string);
        match ca {
            Some(ca) => view! { <p class="mt-1.5 font-mono text-[13px]" data-locked="">{ca}</p> }.into_any(),
            None => view! {
                <p class="mt-1.5 flex items-center gap-1.5 text-xs text-text-muted" data-testid="target-tls-reason">
                    <burrow_ui_kit::Icon name="lock" small=true/>
                    {CA_IN_THE_FILE}
                </p>
            }
            .into_any(),
        }
    };
    view! {
        <div id=fc.id.clone() data-testid="target-tls">
            <Segmented name=format!("{}-choice", fc.id) options=options value=mode label=fc.node.label.clone() off=off on_change=on_mode/>
            {shown}
        </div>
    }
    .into_any()
}
