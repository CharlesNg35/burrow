use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use tracing::warn;

use crate::config::settings::Limit;

static OPEN_FILES: AtomicU64 = AtomicU64::new(u64::MAX);

pub(crate) struct Slots {
    pub(super) open: AtomicUsize,
    max: AtomicUsize,
}

impl Slots {
    pub(crate) fn new(max: usize) -> Slots {
        Slots {
            open: AtomicUsize::new(0),
            max: AtomicUsize::new(max),
        }
    }

    pub(super) fn take(self: &Arc<Self>) -> Option<Slot> {
        self.open
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < self.most()).then_some(n + 1)
            })
            .ok()
            .map(|_| Slot(self.clone()))
    }

    pub(super) fn most(&self) -> usize {
        self.max.load(Ordering::Acquire)
    }

    pub(crate) fn limit(&self, max: usize) {
        self.max.store(max, Ordering::Release);
    }
}

pub(crate) const MAX_WAITING: usize = 4096;

pub(crate) static WAITING_CAP: AtomicUsize = AtomicUsize::new(MAX_WAITING);

pub(super) fn caps(open_files: u64, connections: usize, peers: usize) -> (usize, usize, usize) {
    let budget = usize::try_from(open_files.saturating_sub(512)).unwrap_or(usize::MAX);
    let fit = |share: usize, cap: usize| (budget / 8).saturating_mul(share).max(64).min(cap);
    (fit(3, connections), fit(1, peers), fit(1, MAX_WAITING))
}

#[cfg(unix)]
pub(crate) fn fit_to_open_files(limit: u64) {
    OPEN_FILES.store(limit, Ordering::Relaxed);
    let (_, _, waiting) = caps(limit, 0, 0);
    WAITING_CAP.store(waiting, Ordering::Relaxed);
}

fn wide(n: u32) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

pub(crate) fn fitted(connections: u32, peers: u32) -> (usize, usize) {
    let (connections, peers, _) = caps(
        OPEN_FILES.load(Ordering::Relaxed),
        wide(connections),
        wide(peers),
    );
    (connections, peers)
}

pub(crate) fn client_connection_cap(configured: Limit) -> usize {
    warned("max_connections", configured, fitted(configured.value, 1).0)
}

pub(crate) fn client_peer_cap(configured: Limit) -> usize {
    warned("max_udp_peers", configured, fitted(1, configured.value).1)
}

fn warned(key: &str, configured: Limit, cap: usize) -> usize {
    if let Some(lowered) = lowered(key, configured, cap, OPEN_FILES.load(Ordering::Relaxed)) {
        warn!("{lowered}");
    }
    cap
}

pub(super) fn lowered(key: &str, configured: Limit, cap: usize, open_files: u64) -> Option<String> {
    let value = configured.value;
    (configured.written && cap < wide(value)).then(|| {
        format!(
            "{key} {value} is lowered to {cap} by the open file limit ({open_files}); {}",
            way_out(key, cap, open_files)
        )
    })
}

pub(super) fn way_out(key: &str, cap: usize, open_files: u64) -> String {
    let fewer = format!("set {key} to {cap} or less to silence this, or run several clients");
    if cfg!(target_os = "macos") {
        if open_files < 10240 {
            "install burrow as a service, which sets 10240, the most macOS allows".into()
        } else {
            format!("10240 is the most macOS allows; {fewer}")
        }
    } else if cfg!(target_os = "linux") && open_files < 1048576 {
        "raise the hard limit as root (limits.conf, or LimitNOFILE in a systemd unit), or install burrow as a service, which sets 1048576".into()
    } else {
        format!("raise the hard limit as root, or {fewer}")
    }
}

pub(super) struct Slot(Arc<Slots>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.open.fetch_sub(1, Ordering::AcqRel);
    }
}
