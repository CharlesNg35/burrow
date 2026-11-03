//! Updating this server from burrow admin and burrow mcp: one entry point that checks who may ask and when, then lets the root updater do the rest.
use burrow_admin_proto::msg::ErrorCode;
use burrow_admin_proto::updates::{
    RemoteUpdates, SERVER_UPDATE_NOTE, ServerUpdate, UpdateServer, UpdateServerReply,
};

use super::updates::newer;
use super::{ApiError, Core, Session};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applying {
    Idle,
    Busy,
}

pub const APPLYING: &str = "a config change is being applied; ask again when it is done";

pub fn why_not(own: &ServerUpdate, from: &str, to: &str, applying: Applying) -> Option<String> {
    if own.remote != RemoteUpdates::On {
        return Some(
            own.why_not
                .clone()
                .unwrap_or_else(|| "remote updates are off for this server".into()),
        );
    }
    if own.running() {
        return Some("an update of this server is already running".into());
    }
    if applying == Applying::Busy {
        return Some(APPLYING.into());
    }
    (!newer(to, from)).then(|| format!("it already runs {from}, the latest release"))
}

fn latest(core: &Core, fresh: bool) -> Result<String, ApiError> {
    let known = core.live.latest();
    let latest = match known.version {
        Some(_) if !fresh => known,
        _ => core.live.latest_now(),
    };
    latest.version.ok_or_else(|| {
        ApiError::new(
            ErrorCode::Invalid,
            format!(
                "the latest release is not known yet{}; try again in a moment",
                latest.error.map(|e| format!(" ({e})")).unwrap_or_default()
            ),
        )
    })
}

pub fn update_server(
    core: &Core,
    s: &Session,
    r: &UpdateServer,
    applying: Applying,
) -> Result<UpdateServerReply, ApiError> {
    if !s.writes() {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "read access cannot update the server",
        ));
    }
    ask_server(
        core,
        r,
        &format!("{} ({})", s.visitor, s.peer.ip()),
        applying,
    )
}

pub fn ask_server(
    core: &Core,
    r: &UpdateServer,
    by: &str,
    applying: Applying,
) -> Result<UpdateServerReply, ApiError> {
    let from = core.live.status().version;
    let own = core.live.own_update().unwrap_or_else(|| ServerUpdate {
        why_not: Some("this server cannot update itself from burrow admin".into()),
        ..ServerUpdate::default()
    });
    let to = latest(core, !r.dry_run)?;
    let mut reply = UpdateServerReply {
        from: from.clone(),
        to: to.clone(),
        asked: false,
        why_not: why_not(&own, &from, &to, applying),
        note: SERVER_UPDATE_NOTE.into(),
    };
    if r.dry_run {
        return Ok(reply);
    }
    if let Some(why) = &reply.why_not {
        return Err(ApiError::new(ErrorCode::Conflict, why.clone()));
    }
    core.live
        .update_self(by)
        .map_err(|why| ApiError::new(ErrorCode::Conflict, why))?;
    tracing::info!(
        target: "burrow::admin",
        "{by} asked to update this server {from} -> {to}; its root updater checks and installs the latest release"
    );
    reply.asked = true;
    Ok(reply)
}

#[cfg(test)]
mod tests;
