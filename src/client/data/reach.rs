use std::net::SocketAddr;
use std::sync::Arc;

use tracing::warn;

use super::Dialing;
use crate::base::failure::io_reason;
use crate::config::check::parse_host_port;
use crate::policy::allowlist::{AllowList, Refusal};
use crate::policy::throttle::Throttle;
use crate::wire::protocol::{Connect, Status};
use crate::wire::transport::resolve::Resolver;

pub(super) async fn lookup(
    settings: &Dialing,
    resolver: &Arc<dyn Resolver>,
    connect: &Connect,
) -> std::io::Result<Vec<SocketAddr>> {
    let timed_out = || std::io::Error::new(std::io::ErrorKind::TimedOut, "the lookup timed out");
    if connect.target.parse::<SocketAddr>().is_ok() {
        return tokio::time::timeout(settings.connect_timeout, resolver.resolve(&connect.target))
            .await
            .unwrap_or_else(|_| Err(timed_out()));
    }
    let (slots, max, whose) = match connect.service.is_empty() {
        true => (&VISITOR_LOOKUPS, MAX_LOOKUPS, "visitors"),
        false => (&SERVICE_LOOKUPS, MAX_SERVICE_LOOKUPS, "services"),
    };
    let started = tokio::time::Instant::now();
    let Ok(Ok(held)) = tokio::time::timeout(settings.connect_timeout, slots.acquire()).await else {
        return Err(std::io::Error::other(format!(
            "{max} host name lookups for {whose} are still running"
        )));
    };
    let left = settings.connect_timeout.saturating_sub(started.elapsed());
    let (resolver, target) = (resolver.clone(), connect.target.clone());
    let running = tokio::spawn(async move {
        let _held = held;
        resolver.resolve(&target).await
    });
    match tokio::time::timeout(left, running).await {
        Ok(Ok(resolved)) => resolved,
        Ok(Err(_)) => Err(std::io::Error::other("the lookup stopped")),
        Err(_) => Err(timed_out()),
    }
}

#[cfg(unix)]
pub(super) fn foreign_link_on(path: &std::path::Path) -> Option<std::path::PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let me = rustix::process::geteuid().as_raw();
    path.ancestors().skip(1).find_map(|folder| {
        let m = std::fs::symlink_metadata(folder).ok()?;
        (m.file_type().is_symlink() && m.uid() != 0 && m.uid() != me).then(|| folder.to_path_buf())
    })
}

pub(super) const MAX_LOOKUPS: usize = 128;

pub(super) const MAX_SERVICE_LOOKUPS: usize = 192;

static VISITOR_LOOKUPS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(MAX_LOOKUPS);

static SERVICE_LOOKUPS: tokio::sync::Semaphore =
    tokio::sync::Semaphore::const_new(MAX_SERVICE_LOOKUPS);

pub(super) fn whom(connect: &Connect) -> String {
    match connect.service.as_str() {
        "" => "a visitor".to_string(),
        s => format!("service \"{s}\""),
    }
}

pub(super) async fn permitted(
    settings: &Dialing,
    resolver: &Arc<dyn Resolver>,
    connect: &Connect,
) -> Result<Vec<SocketAddr>, Status> {
    if parse_host_port(&connect.target).is_err() {
        return Err(Status::Invalid);
    }
    let resolved = match lookup(settings, resolver, connect).await {
        Ok(a) => a,
        Err(e) => {
            static UNRESOLVED: Throttle = Throttle::new();
            if let Some(more) = UNRESOLVED.allow() {
                warn!(
                    "{}: cannot resolve {} ({}){more}",
                    label(connect),
                    connect.target,
                    io_reason(&e)
                );
            }
            return Err(if connect.service.is_empty() {
                Status::Denied
            } else {
                Status::Unresolved
            });
        }
    };
    let reach = connect.only.as_deref().map(AllowList::from_rules);
    let reachable: Vec<SocketAddr> = resolved
        .iter()
        .map(|a| SocketAddr::new(a.ip().to_canonical(), a.port()))
        .filter(|a| reach.as_ref().is_none_or(|r| r.permits(*a)))
        .collect();
    if reachable.is_empty() {
        outside_reach(connect);
        return Err(Status::Denied);
    }
    let resolved = reachable;
    let allowed: Vec<SocketAddr> = resolved
        .iter()
        .copied()
        .filter(|a| settings.gate.permits(connect.protocol, *a))
        .collect();
    if allowed.is_empty() {
        static DENIED: Throttle = Throttle::new();
        if let Some(more) = DENIED.allow() {
            let first = resolved[0];
            let whom = whom(connect);
            let why = match settings.gate.refusal(connect.protocol, first) {
                Some(Refusal::Burrow(own)) => {
                    format!("{first} is burrow's own listener {own}, which would loop")
                }
                Some(Refusal::LinkLocal) => format!(
                    "{first} is a link-local or metadata address, which {} must name on its own, for example \"{}\"",
                    settings.list,
                    AllowList::suggestion(first)
                ),
                _ => format!(
                    "{first} is not in the allow list; add \"{}\" to {} to permit it",
                    AllowList::suggestion(first),
                    settings.list
                ),
            };
            warn!(
                "denied connection to {} for {whom}: {why}{more}",
                connect.target
            );
        }
        return Err(Status::Denied);
    }
    Ok(allowed)
}

pub(super) fn outside_reach(connect: &Connect) {
    static OUTSIDE: Throttle = Throttle::new();
    if let Some(more) = OUTSIDE.allow() {
        warn!(
            "denied connection to {} for a visitor: the server limits what it may reach to {}{more}",
            connect.target,
            connect.only.as_deref().unwrap_or_default().join(", ")
        );
    }
}

pub(super) fn label(connect: &Connect) -> String {
    match connect.service.as_str() {
        "" => "visitor connection".to_string(),
        s => format!("service \"{s}\""),
    }
}
