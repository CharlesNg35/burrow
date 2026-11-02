use burrow_admin_proto::updates::RolloutStatus;

use super::*;
use crate::metrics::ClientStatus;

fn job(id: u32, state: UpdateState, detail: &str) -> Option<UpdateStatus> {
    Some(UpdateStatus {
        id,
        from: "0.2.2".into(),
        to: "0.2.3".into(),
        state,
        detail: detail.into(),
        by: "burrow clients on the server".into(),
        secs_ago: 0,
    })
}

fn status(jobs: &[(&str, Option<UpdateStatus>)], rollout: Option<RolloutStatus>) -> Status {
    Status {
        clients: jobs
            .iter()
            .map(|(n, u)| ClientStatus {
                name: n.to_string(),
                connected: true,
                update: u.clone(),
                ..ClientStatus::default()
            })
            .collect(),
        rollout,
        ..Status::default()
    }
}

fn names(n: &[&str]) -> Vec<String> {
    n.iter().map(|s| s.to_string()).collect()
}

#[test]
fn one_client_is_followed_until_it_reports_the_new_version() {
    let before = status(&[("home", job(1, UpdateState::Done, ""))], None);
    let mut w = Watch::new("0.2.3", &names(&["home"]), &before);
    assert_eq!(
        w.step(&before),
        (vec![], false),
        "an older job is not this one"
    );
    let (lines, done) = w.step(&status(
        &[("home", job(2, UpdateState::Downloading, ""))],
        None,
    ));
    assert_eq!(
        (lines, done),
        (
            vec!["home: 0.2.2 → 0.2.3: downloading and checking".to_string()],
            false
        )
    );
    let same = w.step(&status(
        &[("home", job(2, UpdateState::Downloading, ""))],
        None,
    ));
    assert_eq!(same, (vec![], false), "a state is said once");
    let (lines, done) = w.step(&status(&[("home", job(2, UpdateState::Done, ""))], None));
    assert_eq!(lines, ["home: 0.2.2 → 0.2.3: updated"]);
    assert!(done && w.ok());
}

#[test]
fn a_rollback_ends_with_its_reason_and_the_rest_are_named_as_not_updated() {
    let asked = names(&["home", "nas", "pi"]);
    let mut w = Watch::new("0.2.3", &asked, &status(&[], None));
    w.step(&status(&[("home", job(1, UpdateState::Done, ""))], None));
    let rollout = RolloutStatus {
        to: "0.2.3".into(),
        clients: asked.clone(),
        done: names(&["home"]),
        stopped_at: Some("nas".into()),
        finished: true,
        ..RolloutStatus::default()
    };
    let (lines, done) = w.step(&status(
        &[
            ("home", job(1, UpdateState::Done, "")),
            (
                "nas",
                job(
                    2,
                    UpdateState::RolledBack,
                    "did not connect within a minute",
                ),
            ),
        ],
        Some(rollout),
    ));
    assert_eq!(
        lines,
        [
            "nas: 0.2.2 → 0.2.3: rolled back (did not connect within a minute)",
            "stopped: pi not updated"
        ]
    );
    assert!(done);
    assert!(!w.ok());
    assert_eq!(w.results["pi"].state, None);
}
