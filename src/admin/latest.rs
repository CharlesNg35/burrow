use burrow_admin_proto::updates::LatestRelease;

type Check = fn() -> LatestRelease;

static LATEST: std::sync::OnceLock<(Check, Check)> = std::sync::OnceLock::new();

const WAIT: std::time::Duration = std::time::Duration::from_secs(10);

pub fn use_latest(cached: Check, fresh: Check) {
    let _ = LATEST.set((cached, fresh));
}

pub fn cached() -> LatestRelease {
    LATEST.get().map(|(cached, _)| cached()).unwrap_or_default()
}

pub fn fresh() -> LatestRelease {
    LATEST
        .get()
        .map(|(_, fresh)| within(*fresh, WAIT))
        .unwrap_or_default()
}

fn within(check: Check, wait: std::time::Duration) -> LatestRelease {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(check());
    });
    rx.recv_timeout(wait).unwrap_or_else(|_| LatestRelease {
        version: None,
        error: Some(format!(
            "could not check it within {}",
            burrow_syntax::duration::span(wait.as_secs())
        )),
        checked_secs_ago: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slow_check() -> LatestRelease {
        std::thread::sleep(std::time::Duration::from_secs(2));
        LatestRelease {
            version: Some("9.9.9".into()),
            ..Default::default()
        }
    }

    #[test]
    fn a_slow_release_check_answers_in_time_that_it_could_not_check() {
        let slow = within(slow_check, std::time::Duration::from_millis(100));
        assert_eq!(slow.version, None);
        assert_eq!(slow.error.as_deref(), Some("could not check it within 0s"));
        let fast = within(slow_check, std::time::Duration::from_secs(10));
        assert_eq!(fast.version.as_deref(), Some("9.9.9"));
    }
}
