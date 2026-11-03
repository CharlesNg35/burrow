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
