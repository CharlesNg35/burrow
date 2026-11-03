use super::*;
use crate::wire::protocol::{LastUpdate, RunMode};
use burrow_admin_proto::updates::{RemoteUpdates, RunMode as ShownMode};

fn report(version: &str, remote: Remote) -> Report {
    Report {
        version: version.into(),
        os: "linux".into(),
        arch: "arm64".into(),
        run: RunMode::Service,
        remote,
        last: None,
    }
}

fn board(names: &[&str]) -> (Board, Instant) {
    let now = Instant::now();
    let mut b = Board::default();
    for n in names {
        b.reported(n, report("0.2.1", Remote::On), now);
    }
    (b, now)
}

#[test]
fn the_last_report_of_each_client_is_kept() {
    let mut b = Board::default();
    let now = Instant::now();
    assert_eq!(b.build("nas"), None);
    b.reported("nas", report("0.2.1", Remote::Off), now);
    b.reported(
        "nas",
        report("0.2.2", Remote::Blocked("reinstall".into())),
        now,
    );
    let got = b.build("nas").unwrap();
    assert_eq!(got.version, "0.2.2");
    assert_eq!((got.os.as_str(), got.arch.as_str()), ("linux", "arm64"));
    assert_eq!(got.run, ShownMode::Service);
    assert_eq!(got.remote, RemoteUpdates::Blocked);
    assert_eq!(got.why_not.as_deref(), Some("reinstall"));
    b.seated("nas");
    assert_eq!(b.build("nas"), None, "an older client sends no report");
}

#[test]
fn only_a_reporting_client_that_allows_it_is_asked_and_once_at_a_time() {
    let (mut b, now) = board(&["nas"]);
    b.reported("off", report("0.2.1", Remote::Off), now);
    assert!(
        b.begin("old", "0.2.2", "ops", now)
            .unwrap_err()
            .contains("older than 0.2.2")
    );
    assert!(
        b.begin("off", "0.2.2", "ops", now)
            .unwrap_err()
            .contains("does not allow")
    );
    let id = b.begin("nas", "0.2.2", "ops", now).unwrap();
    assert!(
        b.begin("nas", "0.2.2", "ops", now)
            .unwrap_err()
            .contains("already running")
    );
    let job = b.job("nas", now).unwrap();
    assert_eq!(
        (job.id, job.state, job.by.as_str()),
        (id, UpdateState::Asked, "ops")
    );
    assert_eq!((job.from.as_str(), job.to.as_str()), ("0.2.1", "0.2.2"));
}

#[test]
fn an_update_ends_when_the_client_comes_back_with_the_new_version_or_says_it_rolled_back() {
    let (mut b, now) = board(&["nas", "pi"]);
    let id = b.begin("nas", "0.2.2", "ops", now).unwrap();
    b.stepped("nas", id + 7, Step::Failed, "not mine".into(), now);
    assert_eq!(b.job("nas", now).unwrap().state, UpdateState::Asked);
    b.stepped("nas", id, Step::Accepted, "downloading".into(), now);
    b.stepped("nas", id, Step::Restarting, "".into(), now);
    assert_eq!(b.job("nas", now).unwrap().state, UpdateState::Restarting);
    b.reported("nas", report("0.2.2", Remote::On), now);
    assert_eq!(b.job("nas", now).unwrap().state, UpdateState::Done);

    let id = b.begin("pi", "0.2.2", "ops", now).unwrap();
    let mut back = report("0.2.1", Remote::On);
    back.last = Some(LastUpdate {
        id,
        outcome: Outcome::RolledBack,
        from: "0.2.1".into(),
        to: "0.2.2".into(),
        detail: "0.2.2 did not connect within 60 s".into(),
    });
    b.reported("pi", back, now);
    let job = b.job("pi", now).unwrap();
    assert_eq!(job.state, UpdateState::RolledBack);
    assert!(job.detail.contains("did not connect"));
}

#[test]
fn a_silent_client_times_out() {
    let (mut b, now) = board(&["nas", "pi"]);
    b.begin("nas", "0.2.2", "ops", now).unwrap();
    let id = b.begin("pi", "0.2.2", "ops", now).unwrap();
    b.stepped("pi", id, Step::Restarting, "".into(), now);
    b.tick(now + ANSWER + Duration::from_secs(1));
    assert_eq!(b.job("nas", now).unwrap().state, UpdateState::TimedOut);
    assert_eq!(b.job("pi", now).unwrap().state, UpdateState::Restarting);
    b.tick(now + COME_BACK + Duration::from_secs(1));
    assert_eq!(b.job("pi", now).unwrap().state, UpdateState::TimedOut);
    assert_eq!(
        b.job("pi", now).unwrap().detail,
        "no word from it for 15m 1s"
    );
}

#[test]
fn update_all_goes_one_client_at_a_time_and_stops_at_the_first_failure() {
    let (mut b, now) = board(&["a", "b", "c"]);
    b.start_rollout(vec!["a".into(), "b".into(), "c".into()], "0.2.2", "ops")
        .unwrap();
    assert!(b.start_rollout(vec!["a".into()], "0.2.2", "ops").is_err());
    let asks = b.tick(now);
    assert_eq!(asks.len(), 1);
    assert_eq!(asks[0].0, "a");
    assert!(b.tick(now).is_empty(), "b waits until a is done");
    b.reported("a", report("0.2.2", Remote::On), now);
    let asks = b.tick(now);
    assert_eq!(asks.iter().map(|a| a.0.as_str()).collect::<Vec<_>>(), ["b"]);
    b.stepped(
        "b",
        asks[0].1,
        Step::Failed,
        "checksum mismatch".into(),
        now,
    );
    assert!(b.tick(now).is_empty(), "c is never asked");
    let r = b.rollout().unwrap();
    assert_eq!(r.done, ["a"]);
    assert_eq!(r.stopped_at.as_deref(), Some("b"));
    assert!(r.finished);
    assert_eq!(b.job("c", now), None);
    assert!(b.start_rollout(vec!["c".into()], "0.2.2", "ops").is_ok());
}

#[test]
fn update_all_stops_when_the_next_client_cannot_be_asked() {
    let (mut b, now) = board(&["a"]);
    b.start_rollout(vec!["gone".into(), "a".into()], "0.2.2", "ops")
        .unwrap();
    assert!(b.tick(now).is_empty());
    assert_eq!(b.rollout().unwrap().stopped_at.as_deref(), Some("gone"));
    assert_eq!(b.job("a", now), None);
}

#[test]
fn a_request_that_could_not_be_sent_leaves_no_job_behind() {
    let (mut b, now) = board(&["nas"]);
    let id = b.begin("nas", "0.2.2", "ops", now).unwrap();
    b.forget("nas", id + 1);
    assert!(b.job("nas", now).is_some(), "another id is not this job");
    b.forget("nas", id);
    assert_eq!(b.job("nas", now), None);
    assert!(
        b.begin("nas", "0.2.2", "ops", now).is_ok(),
        "a retry is not refused as already running"
    );
}

#[test]
fn a_client_that_installed_a_newer_release_than_asked_counts_as_updated_and_the_rollout_goes_on() {
    let (mut b, now) = board(&["a", "b"]);
    b.start_rollout(vec!["a".into(), "b".into()], "0.2.2", "ops")
        .unwrap();
    let asks = b.tick(now);
    let mut newer = report("0.2.3", Remote::On);
    newer.last = Some(LastUpdate {
        id: asks[0].1,
        outcome: Outcome::Done,
        from: "0.2.1".into(),
        to: "0.2.3".into(),
        detail: String::new(),
    });
    b.reported("a", newer, now);
    let job = b.job("a", now).unwrap();
    assert_eq!(job.state, UpdateState::Done);
    assert_eq!(job.to, "0.2.3", "the version it really runs");
    let asks = b.tick(now);
    assert_eq!(asks.iter().map(|a| a.0.as_str()).collect::<Vec<_>>(), ["b"]);
    let mut stale = report("0.2.1", Remote::On);
    stale.last = Some(LastUpdate {
        id: asks[0].1,
        outcome: Outcome::Done,
        from: "0.2.0".into(),
        to: "0.2.1".into(),
        detail: String::new(),
    });
    b.reported("b", stale, now);
    assert_ne!(
        b.job("b", now).unwrap().state,
        UpdateState::Done,
        "older than asked is not done"
    );
}
