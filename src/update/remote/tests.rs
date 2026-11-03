use super::*;
use crate::wire::protocol::Outcome;

#[test]
fn only_a_newer_release_is_taken_and_prereleases_only_by_prereleases() {
    assert!(allowed_target("0.2.2", "0.2.1").is_ok());
    assert!(allowed_target("v0.2.2", "0.2.1").is_ok());
    let same = allowed_target("0.2.1", "0.2.1").unwrap_err();
    assert!(same.contains("never go back"), "{same}");
    assert!(allowed_target("0.2.0", "0.2.1").is_err());
    let pre = allowed_target("0.3.0-rc.1", "0.2.1").unwrap_err();
    assert!(pre.contains("pre-release"), "{pre}");
    assert!(allowed_target("0.2.1-test.2", "0.2.1-test.1").is_ok());
    assert!(allowed_target("0.2.1", "0.2.1-test.2").is_ok());
    assert!(allowed_target("latest", "0.2.1").is_err());
    assert!(allowed_target("../../etc", "0.2.1").is_err());
}

#[test]
fn a_service_installed_without_remote_updates_says_how_to_allow_them() {
    let mut m = crate::service::stray::bare("burrow-nas");
    m.role = crate::install::Role::Client;
    m.source_config = PathBuf::from("/home/me/client.yaml");
    let why = Mode::Service(Box::new(m)).blocked().unwrap();
    if crate::base::build::release_asset().is_some() {
        assert!(
            why.contains("service install -c /home/me/client.yaml"),
            "{why}"
        );
    }
    assert!(Mode::Container.blocked().unwrap().contains("image"));
}

#[test]
fn a_failure_after_the_swap_counts_as_rolled_back() {
    assert_eq!(
        outcome_of("0.2.2 did not connect within 60 s; restored 0.2.1, and it runs again"),
        Outcome::RolledBack
    );
    assert_eq!(outcome_of("cannot download x"), Outcome::Failed);
}

#[test]
fn the_asked_id_lives_in_the_services_own_folder_not_in_the_shared_instance_folder() {
    let mut m = crate::service::stray::bare("burrow-nas");
    m.role = crate::install::Role::Client;
    m.config = PathBuf::from("/etc/burrow/nas/client.yaml");
    let asked = service_side::asked_file(&m);
    assert!(
        !asked.starts_with(crate::instance::default_dir()),
        "{}",
        asked.display()
    );
    assert_eq!(
        asked.parent(),
        Some(crate::service::helper::own_dir(&m).as_path())
    );
    let dir = tempfile::tempdir().unwrap();
    let result = dir.path().join("result.json");
    record::write(
        &result,
        &Record::new(0, "0.2.2", "0.2.3", &m.config).ended(Outcome::Done, ""),
        0o644,
    )
    .unwrap();
    let shared = dir.path().join("update-asked-planted.json");
    record::write(
        &shared,
        &record::Asked {
            id: 99,
            at: 0,
            from: "0.2.2".into(),
            ..record::Asked::default()
        },
        0o644,
    )
    .unwrap();
    let own = dir.path().join("own-asked.json");
    assert_eq!(
        service_side::last_from(&result, &own).unwrap().id,
        0,
        "a planted file elsewhere is never read"
    );
    record::write(
        &own,
        &record::Asked {
            id: 7,
            at: 0,
            from: "0.2.2".into(),
            ..record::Asked::default()
        },
        0o600,
    )
    .unwrap();
    assert_eq!(service_side::last_from(&result, &own).unwrap().id, 7);
}

#[test]
fn a_result_of_the_job_before_is_not_taken_for_the_one_just_asked() {
    let dir = tempfile::tempdir().unwrap();
    let result = dir.path().join("result.json");
    let own = dir.path().join("asked.json");
    let config = PathBuf::from("/etc/burrow/nas/client.yaml");
    record::write(
        &own,
        &record::Asked {
            id: 2,
            at: 0,
            from: "0.2.2".into(),
            ..record::Asked::default()
        },
        0o600,
    )
    .unwrap();
    let before = Record::new(0, "0.2.1", "0.2.2", &config).ended(Outcome::Done, "");
    record::write(&result, &before, 0o644).unwrap();
    assert_eq!(service_side::last_from(&result, &own).unwrap().id, 0);
    assert!(!service_side::answers(
        &before,
        &record::read(&own).unwrap()
    ));
    let this = Record::new(0, "0.2.2", "0.2.3", &config).ended(Outcome::Failed, "checksum");
    record::write(&result, &this, 0o644).unwrap();
    assert_eq!(service_side::last_from(&result, &own).unwrap().id, 2);
    assert!(service_side::answers(&this, &record::read(&own).unwrap()));
}

fn service(name: &str, role: crate::install::Role, remote: bool) -> Manifest {
    let mut m = crate::service::stray::bare(name);
    m.role = role;
    m.remote_updates = remote;
    m.binary = PathBuf::from("/usr/local/bin/burrow");
    m.source_config = PathBuf::from("/home/me/server.yaml");
    m
}

#[test]
fn a_server_is_trusted_only_by_the_root_owned_install_record_and_its_config_is_never_read() {
    use crate::install::Role;
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("server.yaml");
    std::fs::write(&config, "update:\n  remote: true\n").unwrap();
    let mut off = service("burrow-hub", Role::Server, false);
    off.config = config.clone();
    let why = helper::opted_in(&off).unwrap_err();
    assert!(why.contains("installed without remote updates"), "{why}");
    assert!(
        why.contains("service install -c /home/me/server.yaml"),
        "{why}"
    );
    let mut on = service("burrow-hub", Role::Server, true);
    on.config = dir.path().join("missing").join("server.yaml");
    assert_eq!(helper::opted_in(&on), Ok(()), "server.yaml is not read");
    std::fs::write(&config, "{{ not yaml").unwrap();
    on.config = config;
    assert_eq!(helper::opted_in(&on), Ok(()), "nor parsed");
}

#[test]
fn a_client_still_needs_its_installed_config_to_allow_it() {
    use crate::install::Role;
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("client.yaml");
    let secret = crate::wire::secret::Secret::generate().encode();
    std::fs::write(
        &config,
        format!("server: 127.0.0.1:7835\nname: nas\nsecret: {secret}\nallow: [127.0.0.1/32]\n"),
    )
    .unwrap();
    let mut m = service("burrow-nas", Role::Client, true);
    m.config = config;
    let why = helper::opted_in(&m).unwrap_err();
    assert!(why.contains("remote updates are off in"), "{why}");
}

#[test]
fn a_binary_shared_with_a_service_that_did_not_opt_in_is_never_replaced() {
    use crate::install::Role;
    let me = service("burrow-hub", Role::Server, true);
    let other = service("burrow-nas", Role::Client, false);
    let opted = service("burrow-pi", Role::Client, true);
    let visitor = service("burrow-ops", Role::Visitor, true);
    let why = helper::shared_with_others(&me, vec![me.clone(), other, opted.clone()]).unwrap_err();
    assert!(
        why.starts_with("burrow-nas also run /usr/local/bin/burrow"),
        "{why}"
    );
    assert_eq!(
        helper::shared_with_others(&me, vec![me.clone(), opted]),
        Ok(())
    );
    assert!(
        helper::shared_with_others(&me, vec![me.clone(), visitor]).is_err(),
        "a visitor never opts in"
    );
    let mut elsewhere = service("burrow-x", Role::Client, false);
    elsewhere.binary = PathBuf::from("/opt/burrow");
    assert_eq!(
        helper::shared_with_others(&me, vec![me.clone(), elsewhere]),
        Ok(())
    );
}

#[test]
fn a_server_never_takes_an_older_release_or_a_pre_release_unless_it_runs_one() {
    assert!(allowed_target("0.2.2", "0.2.3").is_err());
    assert!(allowed_target("0.2.3", "0.2.3").is_err());
    assert!(allowed_target("0.2.4-rc.1", "0.2.3").is_err());
    assert!(allowed_target("0.2.4", "0.2.4-rc.1").is_ok());
}

#[test]
fn a_server_run_by_hand_or_in_a_container_or_installed_without_the_opt_in_says_what_to_do() {
    use burrow_admin_proto::updates::RemoteUpdates;
    let (r, why) = server::remote(&Mode::ByHand { binary: None }, false);
    assert_eq!(r, RemoteUpdates::Blocked);
    assert!(why.unwrap().contains("burrow update"));
    let (r, why) = server::remote(&Mode::Container, false);
    assert_eq!(r, RemoteUpdates::Blocked);
    assert!(why.unwrap().contains("update the image"));
    let off = service("burrow-hub", crate::install::Role::Server, false);
    let (r, why) = server::remote(&Mode::Service(Box::new(off.clone())), false);
    assert_eq!(r, RemoteUpdates::Off);
    let why = why.unwrap();
    assert!(
        why.contains("update.remote: true")
            && why.contains("service install -c /home/me/server.yaml"),
        "{why}"
    );
    let (r, written) = server::remote(&Mode::Service(Box::new(off)), true);
    assert_eq!(r, RemoteUpdates::Off, "server.yaml alone turns nothing on");
    assert!(written.unwrap().contains("counts only once root installs"));
}

#[test]
fn the_servers_last_update_joins_who_asked_with_the_root_written_result() {
    use burrow_admin_proto::updates::UpdateState;
    let config = PathBuf::from("/etc/burrow/hub/server.yaml");
    let asked = record::Asked {
        id: 0,
        at: 1000,
        from: "0.2.2".into(),
        by: "ops (203.0.113.7)".into(),
        at_ms: 0,
    };
    let waiting = server::last(None, Some(asked.clone()), 1010).unwrap();
    assert_eq!(waiting.state, UpdateState::Asked);
    assert_eq!(waiting.by, "ops (203.0.113.7)");
    assert!(!waiting.state.finished(), "one job at a time");
    let mut trying = Record::new(0, "0.2.2", "0.2.3", &config);
    trying.at = 1001;
    let now = server::last(Some(trying.clone()), Some(asked.clone()), 1005).unwrap();
    assert_eq!(now.state, UpdateState::Restarting);
    assert_eq!(now.to, "0.2.3");
    let mut back = trying.ended(Outcome::RolledBack, "0.2.3 is not running");
    back.at = 1070;
    let done = server::last(Some(back.clone()), Some(asked.clone()), 1080).unwrap();
    assert_eq!(done.state, UpdateState::RolledBack);
    assert_eq!(done.detail, "0.2.3 is not running");
    assert_eq!(done.by, "ops (203.0.113.7)");
    let mut older = back.clone();
    older.at = 900;
    let stale = server::last(Some(older), Some(asked.clone()), 1000 + 31 * 60).unwrap();
    assert_eq!(stale.state, UpdateState::TimedOut);
    assert!(stale.state.finished());
    let alone = server::last(Some(back), None, 2000).unwrap();
    assert_eq!(alone.by, "");
    assert_eq!(server::last(None, None, 0), None);
}

#[test]
fn a_result_from_earlier_in_the_same_second_does_not_answer_a_new_request() {
    let config = PathBuf::from("/etc/burrow/hub/server.yaml");
    let mut before = Record::new(0, "0.2.2", "0.2.3", &config).ended(Outcome::Failed, "x");
    before.at = 1000;
    before.at_ms = 1_000_100;
    let asked = record::Asked {
        id: 0,
        at: 1000,
        from: "0.2.2".into(),
        by: "ops".into(),
        at_ms: 1_000_900,
    };
    assert!(!service_side::answers(&before, &asked));
    let running = server::last(Some(before.clone()), Some(asked.clone()), 1000).unwrap();
    assert!(!running.state.finished(), "still one job running");
    let mut after = before;
    after.at_ms = 1_000_950;
    assert!(service_side::answers(&after, &asked));
}
