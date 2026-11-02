use std::collections::HashMap;

use burrow_admin_proto::msg::{AuditEntry, ErrorCode, HistoryReply, HistoryRequest};

use super::{AUDIT_RING, ApiError, Core, diff, plan, text, values};

impl Core {
    pub fn history(&self, req: &HistoryRequest) -> Result<HistoryReply, ApiError> {
        let limit = req.limit.unwrap_or(50).clamp(1, AUDIT_RING as u32) as usize;
        let mut entries: Vec<AuditEntry> = self
            .audit
            .lock()
            .expect("audit lock")
            .iter()
            .take(limit)
            .cloned()
            .collect();
        let mut kept: HashMap<String, bool> = HashMap::new();
        for e in &mut entries {
            e.revertible = *kept
                .entry(e.rev.clone())
                .or_insert_with(|| self.store.version(&e.rev).is_some());
        }
        let missing = match (req.entry, &req.diff) {
            (Some(id), _) => Some(format!("no history entry {id}")),
            (None, Some(rev)) => Some(format!("no history entry for {rev}")),
            (None, None) => None,
        };
        let wanted = |e: &AuditEntry| match (req.entry, &req.diff) {
            (Some(id), _) => e.id == id,
            (None, Some(rev)) => e.rev == *rev,
            (None, None) => false,
        };
        let mut files = Vec::new();
        let diff = match missing {
            Some(missing) => {
                let entry = self
                    .audit
                    .lock()
                    .expect("audit lock")
                    .iter()
                    .find(|e| wanted(e))
                    .cloned()
                    .ok_or_else(|| ApiError::new(ErrorCode::BadRequest, missing))?;
                let after = self.store.version(&entry.rev);
                let before = self.store.version(&entry.base);
                match (before, after) {
                    (Some(b), Some(a)) => {
                        let texts: Vec<&str> = b
                            .list
                            .iter()
                            .chain(&a.list)
                            .map(|d| d.text.as_str())
                            .collect();
                        let hashes = values::hashes_in(&texts, &self.schema().root);
                        let headers = values::header_values_in(&texts);
                        let mask = |l: &str| {
                            text::mask_secrets(&values::veil_line(l, &hashes, &headers), &[])
                        };
                        files = plan::files::diffs(&b, &a, &mask);
                        Some(diff::unified(&b.main().text, &a.main().text, &mask))
                    }
                    _ => None,
                }
            }
            None => None,
        };
        Ok(HistoryReply {
            entries,
            diff,
            files,
        })
    }
}
