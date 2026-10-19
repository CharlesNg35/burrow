use burrow_admin_proto::msg::{ConnectInfo, ErrorCode};
use burrow_admin_proto::plan::Snippets;

use super::plan::Computed;
use super::{ApiError, Core, Session};

pub fn connect_info(_core: &Core, _s: &Session, req: &ConnectInfo) -> Result<Snippets, ApiError> {
    Err(ApiError::new(
        ErrorCode::UnknownRequest,
        format!(
            "connection details for {} {} are not available yet",
            req.kind, req.name
        ),
    ))
}

pub fn after_apply(_core: &Core, _c: &Computed) -> Vec<Snippets> {
    Vec::new()
}
