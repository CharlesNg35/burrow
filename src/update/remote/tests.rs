use super::*;

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
