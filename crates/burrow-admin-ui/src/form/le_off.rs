use burrow_admin_proto::Path;
use burrow_admin_proto::codes::NO_CERTIFICATE;
use burrow_config::{ServerConfig, WebConfig};
use leptos::prelude::*;

use crate::store::store;

use super::editing_now;

fn is_letsencrypt(path: &[String]) -> bool {
    matches!(path, [w, l] if w == ServerConfig::WEB.key() && l == WebConfig::LETSENCRYPT.key())
}

pub(super) fn note_for(path: &Path) -> Option<AnyView> {
    if !is_letsencrypt(path) {
        return None;
    }
    let e = store().edit;
    let editing = editing_now();
    let p = path.clone();
    let hosts = Memo::new(move |_| {
        if e.value(&p).is_some_and(|v| !v.is_null()) {
            return Vec::new();
        }
        e.plan.with(|plan| {
            let mut out: Vec<String> = plan
                .as_deref()
                .map(|plan| {
                    plan.diags
                        .iter()
                        .filter(|d| d.is_error() && d.code == NO_CERTIFICATE)
                        .flat_map(|d| d.subjects.iter().cloned())
                        .collect()
                })
                .unwrap_or_default();
            out.sort();
            out.dedup();
            out
        })
    });
    Some(
        (move || {
            let hosts = hosts.get();
            (editing.try_get().unwrap_or(false) && !hosts.is_empty()).then(|| {
                let names = hosts.join(", ");
                let text = format!(
                    "{names} would have no certificate. Turn Let's Encrypt back on, or give {} your own certificate.",
                    if hosts.len() == 1 { "it" } else { "them" }
                );
                view! { <div class="px-4 pt-3">{burrow_ui_kit::field_message(burrow_ui_kit::Tone::Warning, text)}</div> }
            })
        })
        .into_any(),
    )
}
