use burrow_admin_proto::diag::{Diag, Severity};
use serde_json::Value;

use super::{Core, Session};

pub fn notice(core: &Core, s: &Session, raw: Option<&Value>) -> Option<Diag> {
    if !s.older_ui() {
        return None;
    }
    let spots = burrow_config::older::v0_1_2_cannot_edit(&core.schema().root, raw?);
    if spots.is_empty() {
        return None;
    }
    let version = crate::admin::hooks::version(env!("CARGO_PKG_VERSION"));
    let (message, help) = burrow_config::older::older_admin(&version, &spots);
    Some(Diag {
        severity: Severity::Warning,
        code: burrow_admin_proto::codes::OLDER_ADMIN.into(),
        message,
        path: None,
        also: Vec::new(),
        label: None,
        help: Some(help),
        line: None,
        col: None,
        before: false,
        subjects: Vec::new(),
        file: None,
    })
}
