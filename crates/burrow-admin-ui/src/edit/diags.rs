use std::collections::BTreeMap;
use std::sync::Arc;

use burrow_admin_logic::changeset::{Changeset, Mark};
use burrow_admin_logic::entity_errors::{self, Errors};
use burrow_admin_proto::diag::{Diag, Severity};
use burrow_admin_proto::plan::Plan;
use burrow_admin_proto::schema::Schema;
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

pub(super) type Seen = Option<(Mark, Arc<Schema>)>;

pub(super) fn related_errors(
    draft: RwSignal<Changeset>,
    prev: Option<&Errors>,
    seen: StoredValue<Seen>,
) -> Errors {
    let Some(schema) = crate::store::try_store().and_then(|s| s.schema.get()) else {
        seen.set_value(None);
        return BTreeMap::new();
    };
    draft.with(|d| {
        let out = seen.with_value(|was| {
            let (mark, _) = was.as_ref().filter(|(_, s)| Arc::ptr_eq(s, &schema))?;
            let changed = d.changed_since(*mark)?;
            let mut out = prev?.clone();
            entity_errors::refresh(&schema, d.draft(), &mut out, changed);
            Some(out)
        });
        let out = out.unwrap_or_else(|| entity_errors::entity_errors(&schema, d.draft()));
        seen.set_value(Some((d.mark(), schema)));
        out
    })
}

pub fn severity_tone(d: &Diag) -> burrow_ui_kit::Tone {
    match d.severity {
        Severity::Error => burrow_ui_kit::Tone::Danger,
        Severity::Warning => burrow_ui_kit::Tone::Warning,
        Severity::Info => burrow_ui_kit::Tone::Info,
    }
}
