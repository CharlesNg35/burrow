use burrow_admin_proto::serde_json::{Value, json};
use burrow_config::{Entry, ServiceEntry, TargetCa, TargetUnchecked, WebSite};
use leptos::prelude::*;

use crate::store::store;

use super::FieldCtx;

const OFF: &str = "off";
const PUBLIC: &str = "public";
const CA: &str = "own-ca";
const UNCHECKED: &str = "unchecked";

pub(super) fn is_target_tls(fc: &FieldCtx) -> bool {
    matches!(fc.spath.as_slice(), [s, _, w, t] if s == ServiceEntry::SECTION && w == ServiceEntry::WEB.key() && t == WebSite::TARGET_TLS.key())
}

fn mode_of(v: Option<&Value>) -> &'static str {
    match v {
        Some(Value::Bool(true)) => PUBLIC,
        Some(Value::Object(m))
            if m.get(TargetUnchecked::INSECURE_SKIP_VERIFY.key()) == Some(&Value::Bool(true)) =>
        {
            UNCHECKED
        }
        Some(Value::Object(_)) => CA,
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
        (CA.to_string(), "Checked by my CA".to_string(), false),
        (UNCHECKED.to_string(), "Not checked".to_string(), false),
    ];
    let p2 = fc.path.clone();
    let touched = fc.touched;
    let on_mode = move |m: String| {
        let v = match m.as_str() {
            PUBLIC => Some(Value::Bool(true)),
            UNCHECKED => Some(json!({ TargetUnchecked::INSECURE_SKIP_VERIFY.key(): true })),
            CA if mode_of(e.value(&p2).as_ref()) == CA => return,
            CA => Some(json!({})),
            _ => None,
        };
        touched.set(true);
        e.set(&p2, v);
    };
    let ca_fields = crate::uploads::object_of(&fc.node, TargetCa::CA.key());
    let fc2 = fc.clone();
    let shown = move || {
        (mode.get() == CA).then(|| {
            ca_fields
                .as_ref()
                .map(|o| crate::uploads::certificate_source(&fc2, o))
        })
    };
    let choice = super::Choice {
        options,
        mode,
        on_change: Callback::new(on_mode),
    };
    let below = view! { <div class="mt-1.5">{shown}</div> };
    super::choice_box(&fc, Some("target-tls"), choice, below)
}
