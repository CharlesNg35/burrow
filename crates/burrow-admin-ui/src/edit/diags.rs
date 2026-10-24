use std::collections::BTreeMap;

use burrow_admin_logic::changeset::Changeset;
use burrow_admin_proto::Path;
use burrow_admin_proto::diag::{Diag, Severity};
use burrow_admin_proto::plan::Plan;
use burrow_admin_proto::schema::SemType;
use burrow_admin_proto::serde_json::Value;
use leptos::prelude::*;

pub fn diags_at<'a>(plan: Option<&'a Plan>, path: &[String], items: bool) -> Vec<&'a Diag> {
    plan.map(|p| {
        p.diags
            .iter()
            .filter(|d| {
                if items {
                    d.within(path)
                } else {
                    d.concerns(path)
                }
            })
            .collect()
    })
    .unwrap_or_default()
}

pub(super) fn related_errors(draft: RwSignal<Changeset>) -> BTreeMap<Path, String> {
    let Some(schema) = crate::store::try_store().and_then(|s| s.schema.get()) else {
        return BTreeMap::new();
    };
    draft.with(|d| entity_errors(&schema, d.draft()))
}

fn entity_errors(
    schema: &burrow_admin_proto::schema::Schema,
    doc: &Value,
) -> BTreeMap<Path, String> {
    let mut out = BTreeMap::new();
    for info in &schema.entities {
        let Some(node) = schema.root.at(&info.path) else {
            continue;
        };
        let SemType::Map(m) = &burrow_admin_logic::tree::inner(node).sem else {
            continue;
        };
        let Some(items) =
            burrow_admin_proto::value::get(doc, &info.path).and_then(Value::as_object)
        else {
            continue;
        };
        for (name, value) in items {
            for (key, msg) in burrow_admin_logic::relations::entity_problems(&m.value, value) {
                let mut p = info.path.clone();
                p.extend([name.clone(), key]);
                out.entry(p).or_insert(msg);
            }
        }
    }
    out
}

pub fn severity_tone(d: &Diag) -> burrow_ui_kit::Tone {
    match d.severity {
        Severity::Error => burrow_ui_kit::Tone::Danger,
        Severity::Warning => burrow_ui_kit::Tone::Warning,
        Severity::Info => burrow_ui_kit::Tone::Info,
    }
}
