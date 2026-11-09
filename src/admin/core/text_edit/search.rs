use burrow_admin_proto::text_search::{SearchReply, SearchText, find};

use super::{mask, named};
use crate::admin::core::{ApiError, Core, text};

pub fn search_text(core: &Core, r: &SearchText) -> Result<SearchReply, ApiError> {
    let docs = core.docs()?;
    let root = &core.schema().root;
    let mut shown = Vec::new();
    let mut unsearchable = Vec::new();
    for d in &docs.list {
        match mask::mask(
            named(core, &docs, &d.name),
            &d.text,
            root,
            &core.text_edit.ids,
        ) {
            Ok(m) => shown.push((d.name.as_str(), m.text)),
            Err(_) => unsearchable.push(d.name.clone()),
        }
    }
    let files: Vec<(&str, &str)> = shown.iter().map(|(n, t)| (*n, t.as_str())).collect();
    let (matches, more) = find(&files, &r.query, r.limit());
    Ok(SearchReply {
        rev: text::docs_rev(&docs),
        matches,
        more,
        unsearchable,
    })
}
