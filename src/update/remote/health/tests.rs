use burrow_admin_proto::status::{ClientStatus, PortWait, ServiceStatus};

use super::server::{Seen, back, judge};
use crate::metrics::Status;

fn status(version: &str) -> Status {
    Status {
        version: version.into(),
        services: vec![ServiceStatus {
            name: "web".into(),
            ..ServiceStatus::default()
        }],
        clients: ["home", "nas"]
            .into_iter()
            .map(|n| ClientStatus {
                name: n.into(),
                connected: true,
                ..ClientStatus::default()
            })
            .collect(),
        ..Status::default()
    }
}

#[test]
fn a_server_is_healthy_when_it_reports_the_new_version_and_every_listener_listens() {
    let s = status("0.2.3");
    assert_eq!(judge(&s, "0.2.3", &Seen::default()), Ok(()));
    let old = judge(&status("0.2.2"), "0.2.3", &Seen::default()).unwrap_err();
    assert_eq!(old, "it reports 0.2.2, not 0.2.3");
}

#[test]
fn a_listener_that_stopped_listening_fails_the_check_but_one_that_was_down_before_does_not() {
    let mut s = status("0.2.3");
    s.services[0].not_listening = Some("address in use".into());
    let why = judge(&s, "0.2.3", &Seen::default()).unwrap_err();
    assert!(
        why.contains("service \"web\"") && why.contains("address in use"),
        "{why}"
    );
    let before = Seen {
        quiet: ["web".to_string()].into(),
        ..Seen::default()
    };
    assert_eq!(judge(&s, "0.2.3", &before), Ok(()));
    s.services[0].not_listening = None;
    s.web_waiting = Some(PortWait {
        key: "web.https".into(),
        addr: "0.0.0.0:443".into(),
        reason: "permission denied".into(),
        ..PortWait::default()
    });
    let web = judge(&s, "0.2.3", &Seen::default()).unwrap_err();
    assert!(
        web.contains("web.https 0.0.0.0:443: permission denied"),
        "{web}"
    );
    let waited = Seen {
        web_waiting: true,
        ..Seen::default()
    };
    assert_eq!(judge(&s, "0.2.3", &waited), Ok(()));
}

#[test]
fn clients_coming_back_are_counted_for_the_result_only() {
    let before = Seen::of(&status("0.2.2"));
    assert_eq!(before.clients.len(), 2);
    let mut now = status("0.2.3");
    assert_eq!(back(&before, &now), "2 of 2 clients connected again");
    now.clients[1].connected = false;
    let some = back(&before, &now);
    assert!(
        some.starts_with("1 of 2 clients connected again so far"),
        "{some}"
    );
    assert_eq!(
        judge(&now, "0.2.3", &before),
        Ok(()),
        "never a reason to roll back"
    );
    assert_eq!(back(&Seen::default(), &now), "");
}

#[test]
fn every_role_that_updates_has_a_health_check_and_a_visitor_has_none() {
    use crate::install::Role;
    assert!(super::of(Role::Client).is_some());
    assert!(super::of(Role::Server).is_some());
    assert!(super::of(Role::Visitor).is_none());
}
