use burrow_admin_proto::check::{CheckText, too_large};
use burrow_admin_proto::msg::ErrorCode;

use super::{ApiError, Core, Reply, plan};
use crate::config::text_check::{self, Reads};

pub fn check(core: &Core, r: &CheckText) -> Reply {
    if let Some(why) = too_large(r.text.len()) {
        return Err(ApiError::new(ErrorCode::TooLarge, why));
    }
    let outcome = plan::load_as(
        core,
        &r.text,
        text_check::role(r.role),
        text_check::format_of(&r.text),
    );
    let values: Vec<String> = crate::config::env::referenced_env(&r.text)
        .iter()
        .filter_map(|n| core.env.get(n))
        .collect();
    Ok(text_check::report_json(
        outcome.report,
        &r.text,
        r.scope,
        Reads::Here,
        &values,
    ))
}
