use std::sync::atomic::{AtomicU64, Ordering};

use burrow_admin_proto::Path;

const KEEP: usize = 1024;

static EPOCHS: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mark {
    epoch: u64,
    at: usize,
}

#[derive(Debug)]
pub(super) struct ChangeLog {
    epoch: u64,
    paths: Vec<Path>,
}

impl ChangeLog {
    pub(super) fn fresh() -> Self {
        ChangeLog {
            epoch: EPOCHS.fetch_add(1, Ordering::Relaxed),
            paths: Vec::new(),
        }
    }

    pub(super) fn note(&mut self, path: &[String]) {
        if self.paths.len() >= KEEP {
            self.everything();
        } else {
            self.paths.push(path.to_vec());
        }
    }

    pub(super) fn everything(&mut self) {
        *self = ChangeLog::fresh();
    }

    pub(super) fn mark(&self) -> Mark {
        Mark {
            epoch: self.epoch,
            at: self.paths.len(),
        }
    }

    pub(super) fn since(&self, m: Mark) -> Option<&[Path]> {
        (m.epoch == self.epoch)
            .then(|| self.paths.get(m.at..))
            .flatten()
    }
}

impl Clone for ChangeLog {
    fn clone(&self) -> Self {
        ChangeLog::fresh()
    }
}

impl PartialEq for ChangeLog {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}
