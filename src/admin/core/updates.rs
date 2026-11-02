use burrow_admin_proto::msg::ErrorCode;
use burrow_admin_proto::status::ClientStatus;
use burrow_admin_proto::updates::{
    FIRST_REPORTING, RemoteUpdates, RunMode, Skipped, UpdateClients, UpdateClientsReply, hand_hint,
    remote_text,
};

use super::{ApiError, Core, Session};
use crate::metrics::Status;

fn version(v: &str) -> Option<semver::Version> {
    semver::Version::parse(v.trim().trim_start_matches('v')).ok()
}

pub fn newer(target: &str, than: &str) -> bool {
    match (version(target), version(than)) {
        (Some(t), Some(c)) => t > c,
        _ => false,
    }
}

pub fn why_not(c: &ClientStatus, target: &str) -> Option<String> {
    if c.local {
        return Some("it is this server".into());
    }
    if !c.connected {
        return Some("it is offline".into());
    }
    let Some(b) = &c.build else {
        return Some(format!(
            "it is older than {FIRST_REPORTING}: {}",
            hand_hint()
        ));
    };
    if b.run == RunMode::Container {
        return Some("it runs in a container: update its image".into());
    }
    match b.remote {
        RemoteUpdates::On => {}
        RemoteUpdates::Off => {
            return Some(
                "remote updates are off in its config (update: remote: true in client.yaml)".into(),
            );
        }
        _ => return Some(format!("remote updates are {}", remote_text(b))),
    }
    if c.update.as_ref().is_some_and(|u| !u.state.finished()) {
        return Some("an update is already running".into());
    }
    if !newer(target, &b.version) {
        return Some(format!("it already runs {}", b.version));
    }
    None
}

pub fn offer(status: &mut Status) {
    let Some(latest) = status.latest.as_ref().and_then(|l| l.version.clone()) else {
        return;
    };
    for c in &mut status.clients {
        c.available = c
            .build
            .as_ref()
            .filter(|b| newer(&latest, &b.version))
            .map(|_| latest.clone());
        c.blocked = c.available.as_ref().and_then(|v| why_not(c, v));
    }
}

fn target(core: &Core, wanted: Option<&str>, status: &Status) -> Result<String, ApiError> {
    let v = match wanted
        .map(str::trim)
        .filter(|v| !v.is_empty() && *v != "latest")
    {
        Some(v) => v.trim_start_matches('v').to_string(),
        None => {
            let latest = status.latest.clone().unwrap_or_else(|| core.live.latest());
            latest.version.ok_or_else(|| {
                ApiError::new(
                    ErrorCode::Invalid,
                    format!(
                        "the latest release is not known yet{}; try again in a moment or name a version",
                        latest.error.map(|e| format!(" ({e})")).unwrap_or_default()
                    ),
                )
            })?
        }
    };
    if version(&v).is_none() || v.contains(['+', ' ']) {
        return Err(ApiError::new(
            ErrorCode::BadRequest,
            format!("\"{v}\" is not a release number like 1.2.3"),
        ));
    }
    Ok(v)
}

pub fn update_clients(
    core: &Core,
    s: &Session,
    r: &UpdateClients,
) -> Result<UpdateClientsReply, ApiError> {
    if !s.writes() {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "read access cannot update clients",
        ));
    }
    let status = core.status();
    let to = target(core, r.version.as_deref(), &status)?;
    let named: Vec<&ClientStatus> = if r.all {
        status.clients.iter().filter(|c| !c.local).collect()
    } else {
        r.clients
            .iter()
            .filter_map(|n| status.clients.iter().find(|c| &c.name == n))
            .collect()
    };
    let mut reply = UpdateClientsReply {
        to: to.clone(),
        ..UpdateClientsReply::default()
    };
    for n in r
        .clients
        .iter()
        .filter(|n| !status.clients.iter().any(|c| &c.name == *n))
    {
        reply.skipped.push(Skipped {
            client: n.clone(),
            why: "there is no client with this name".into(),
        });
    }
    for c in named {
        match why_not(c, &to) {
            None => reply.asked.push(c.name.clone()),
            Some(why) => reply.skipped.push(Skipped {
                client: c.name.clone(),
                why,
            }),
        }
    }
    if r.dry_run {
        return Ok(reply);
    }
    let by = format!("{} ({})", s.visitor, s.peer.ip());
    let started = match reply.asked.as_slice() {
        [] => return Ok(reply),
        [one] if !r.all => core.live.ask_update(one, &to, &by),
        _ => core.live.update_all(reply.asked.clone(), &to, &by),
    };
    started.map_err(|why| ApiError::new(ErrorCode::Conflict, why))?;
    tracing::info!(
        target: "burrow::admin",
        visitor = %s.visitor,
        from = %s.peer.ip(),
        "asked {} to update to {to}",
        reply.asked.join(", ")
    );
    Ok(reply)
}

#[cfg(test)]
mod tests;
