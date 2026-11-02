//! Client updates in the admin core: which clients may take the latest release, and the one entry point that asks them.

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

fn target(
    core: &Core,
    wanted: Option<&str>,
    status: &Status,
    fresh: bool,
) -> Result<String, ApiError> {
    let shown = status.latest.as_ref().and_then(|l| l.version.clone());
    let latest = match status.latest.clone() {
        Some(known) if !fresh && known.version.is_some() => known,
        _ => core.live.latest_now(),
    };
    let known = latest.version.ok_or_else(|| {
        ApiError::new(
            ErrorCode::Invalid,
            format!(
                "the latest release is not known yet{}; try again in a moment",
                latest.error.map(|e| format!(" ({e})")).unwrap_or_default()
            ),
        )
    })?;
    match wanted
        .map(|v| v.trim().trim_start_matches('v'))
        .filter(|v| !v.is_empty() && *v != "latest")
    {
        Some(v) if v != known && shown.as_deref() != Some(v) => Err(ApiError::new(
            ErrorCode::BadRequest,
            format!("clients update only to the latest release, {known}; \"{v}\" is not it"),
        )),
        _ => Ok(known),
    }
}

pub const ON_THE_SERVER: &str = "burrow clients on the server";

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
    ask_clients(core, r, &format!("{} ({})", s.visitor, s.peer.ip()))
}

pub fn ask_clients(
    core: &Core,
    r: &UpdateClients,
    by: &str,
) -> Result<UpdateClientsReply, ApiError> {
    let mut seen = std::collections::HashSet::new();
    let unique = UpdateClients {
        clients: r
            .clients
            .iter()
            .filter(|n| seen.insert(n.as_str()))
            .cloned()
            .collect(),
        ..r.clone()
    };
    let r = &unique;
    let status = core.status();
    let to = target(core, r.version.as_deref(), &status, !r.dry_run)?;
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
    let started = match reply.asked.as_slice() {
        [] => return Ok(reply),
        [one] if !r.all => core.live.ask_update(one, &to, by),
        _ => core.live.update_all(reply.asked.clone(), &to, by),
    };
    started.map_err(|why| ApiError::new(ErrorCode::Conflict, why))?;
    tracing::info!(
        target: "burrow::admin",
        "{by} asked {} to update to {to}",
        reply.asked.join(", ")
    );
    Ok(reply)
}

#[cfg(test)]
mod tests;
