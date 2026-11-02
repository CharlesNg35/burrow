use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use burrow_admin_proto::updates::LatestRelease;

const FRESH: Duration = Duration::from_secs(6 * 3600);
const RETRY: Duration = Duration::from_secs(15 * 60);

struct Known {
    at: Instant,
    found: Result<String, String>,
}

static KNOWN: Mutex<Option<Known>> = Mutex::new(None);
static CHECKING: AtomicBool = AtomicBool::new(false);

pub fn version_in(sums: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let name = line.split_whitespace().nth(1)?.trim_start_matches('*');
        let rest = name.strip_prefix("burrow-")?;
        crate::base::build::ASSETS.iter().find_map(|asset| {
            let v = rest
                .strip_suffix(&format!("-{asset}.tar.gz"))
                .or_else(|| rest.strip_suffix(&format!("-{asset}.zip")))?;
            super::release_version(v).map(|_| v.to_string())
        })
    })
}

fn check() -> Result<String, String> {
    let (_, sums) = super::fetch_sums(&super::releases(), None).map_err(|f| f.message)?;
    version_in(&sums).ok_or_else(|| "the latest release lists no burrow build".to_string())
}

pub fn cached() -> LatestRelease {
    let known = KNOWN.lock().expect("latest lock");
    let stale = match &*known {
        None => true,
        Some(k) => k.at.elapsed() > if k.found.is_ok() { FRESH } else { RETRY },
    };
    let shown = known.as_ref().map(|k| LatestRelease {
        version: k.found.as_ref().ok().cloned(),
        error: k.found.as_ref().err().cloned(),
        checked_secs_ago: k.at.elapsed().as_secs(),
    });
    drop(known);
    if stale && !CHECKING.swap(true, Ordering::AcqRel) {
        std::thread::spawn(|| {
            let found = check();
            *KNOWN.lock().expect("latest lock") = Some(Known {
                at: Instant::now(),
                found,
            });
            CHECKING.store(false, Ordering::Release);
        });
    }
    shown.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_latest_version_is_read_from_the_checksum_list() {
        let sums = format!(
            "{0}  burrow-0.2.2-linux-amd64.tar.gz\n{0}  burrow-0.2.2-macos-arm64.tar.gz\n{0}  burrow-0.2.2-windows-amd64.zip\n",
            "a".repeat(64)
        );
        assert_eq!(version_in(&sums).as_deref(), Some("0.2.2"));
        let pre = format!(
            "{}  burrow-0.3.0-rc.1-linux-riscv64.tar.gz\n",
            "b".repeat(64)
        );
        assert_eq!(version_in(&pre).as_deref(), Some("0.3.0-rc.1"));
        assert_eq!(version_in("nothing here"), None);
    }
}
