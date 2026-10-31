use std::path::Path;

use super::*;
use crate::tls::CertPhase;

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn pem_pair(names: &[&str], days: i64) -> (String, String) {
    let key = rcgen::KeyPair::generate().unwrap();
    let mut p = rcgen::CertificateParams::new(s(names)).unwrap();
    let now = time::OffsetDateTime::now_utc();
    p.not_before = now - time::Duration::days(1);
    p.not_after = now + time::Duration::days(days);
    (p.self_signed(&key).unwrap().pem(), key.serialize_pem())
}

#[test]
fn ip_orders_ask_for_shortlived_when_offered_and_a_missing_profile_is_an_error() {
    let le = s(&["classic", "shortlived", "tlsserver"]);
    let ip = s(&["203.0.113.10"]);
    let dns = s(&["blog.example.com"]);
    assert_eq!(
        choose_profile(&le, &ip, None).unwrap().as_deref(),
        Some("shortlived")
    );
    assert_eq!(
        choose_profile(&le, &ip, Some("tlsserver"))
            .unwrap()
            .as_deref(),
        Some("shortlived")
    );
    assert_eq!(choose_profile(&s(&["default"]), &ip, None).unwrap(), None);
    assert_eq!(choose_profile(&le, &dns, None).unwrap(), None);
    assert_eq!(
        choose_profile(&le, &dns, Some("tlsserver"))
            .unwrap()
            .as_deref(),
        Some("tlsserver")
    );
    assert_eq!(
        choose_profile(&s(&["default", "shortlived"]), &dns, Some("tlsserver")).unwrap_err(),
        "the certificate service does not offer the profile \"tlsserver\"; it offers: default, shortlived"
    );
    assert!(
        choose_profile(&[], &dns, Some("x"))
            .unwrap_err()
            .ends_with("it offers: none")
    );
}

#[test]
fn a_new_certificate_must_parse_match_its_key_and_cover_every_name() {
    let (chain, key) = pem_pair(&["a.example.com", "b.example.com"], 90);
    let held = Held::parse(&chain, &key, &s(&["a.example.com", "b.example.com"])).unwrap();
    assert!(held.not_after > held.not_before);
    let e = Held::parse(&chain, &key, &s(&["a.example.com", "c.example.com"])).unwrap_err();
    assert_eq!(e, "the new certificate does not cover \"c.example.com\"");
    let (_, other) = pem_pair(&["a.example.com"], 90);
    assert!(Held::parse(&chain, &other, &s(&["a.example.com"])).is_err());
    assert!(Held::parse("garbage", &key, &s(&["a.example.com"])).is_err());
}

#[test]
fn a_stored_certificate_is_used_only_for_the_directory_that_issued_it() {
    let state = tempfile::tempdir().unwrap();
    let (chain_pem, key_pem) = pem_pair(&["blog.example.com"], 90);
    let stored = storage::Stored {
        chain_pem,
        key_pem,
        meta: storage::Meta {
            directory: "https://acme.test/dir".into(),
            names: s(&["blog.example.com"]),
            issued: 1,
        },
    };
    let blog = s(&["blog.example.com"]);
    storage::save(&storage::cert_dir(state.path(), &blog), &stored).unwrap();
    let now = now_secs();
    assert!(Held::find(state.path(), "https://acme.test/dir", &blog, now).is_some());
    assert!(Held::find(state.path(), super::super::LETSENCRYPT_STAGING, &blog, now).is_none());
    assert!(
        Held::find(
            state.path(),
            "https://acme.test/dir",
            &s(&["shop.example.com"]),
            now
        )
        .is_none()
    );
}

#[test]
fn a_group_orders_when_it_has_nothing_its_names_changed_or_its_time_came() {
    let (chain, key) = pem_pair(&["a.example.com"], 90);
    let held = Held::parse(&chain, &key, &s(&["a.example.com"])).unwrap();
    let now = now_secs();
    assert!(Group::new(s(&["a.example.com"]), None).needs_order(now));
    let mut g = Group::new(s(&["a.example.com"]), Some(held));
    assert!(!g.needs_order(now));
    assert_eq!(g.view(&[CertCheck::Tls]).phase, CertPhase::Ok);
    g.planned = Some(now - 1);
    assert!(g.needs_order(now));
    g.planned = None;
    g.rename(&s(&["a.example.com", "www.example.com"]));
    assert!(g.needs_order(now));
    g.last_error = Some("refused".into());
    let v = g.view(&[CertCheck::Http]);
    assert_eq!(
        v.phase,
        CertPhase::Failing {
            reason: "refused".into()
        }
    );
    assert_eq!(v.check, Some(CertCheck::Http));
    assert!(v.expires.is_some() && v.next_renewal.is_some());
    let pending = Group::new(s(&["a.example.com"]), None).view(&[]);
    assert_eq!((pending.phase, pending.expires), (CertPhase::Pending, None));
    assert!(pending.next_renewal.is_some(), "the next try shows");
}

#[test]
fn an_ari_id_becomes_a_certificate_identifier() {
    let id = cert_id("aYhba4dGQEHhs3uEe6CuLN4ByNQ.AIdlQyE").unwrap();
    assert_eq!(id.to_string(), "aYhba4dGQEHhs3uEe6CuLN4ByNQ.AIdlQyE");
    assert!(cert_id("nodot").is_none());
}

#[test]
fn a_profile_the_service_does_not_offer_is_told_apart_from_other_failures() {
    let err = choose_profile(&s(&["default"]), &s(&["blog.example.com"]), Some("x")).unwrap_err();
    assert!(err.starts_with(NOT_OFFERED), "{err}");
    let mut g = Group::new(s(&["blog.example.com"]), None);
    g.last_error = Some(err);
    g.code = Some(PROFILE_NOT_OFFERED);
    assert_eq!(g.view(&[]).code, Some(PROFILE_NOT_OFFERED));
    g.last_error = None;
    assert_eq!(
        g.view(&[]).code,
        None,
        "a code without a failure is not shown"
    );
}

fn save_pair(state: &Path, names: &[&str], days: i64) -> String {
    let (chain_pem, key_pem) = pem_pair(names, days);
    let stored = storage::Stored {
        chain_pem: chain_pem.clone(),
        key_pem,
        meta: storage::Meta {
            directory: "https://acme.test/dir".into(),
            names: s(names),
            issued: 1,
        },
    };
    storage::save(&storage::cert_dir(state, &s(names)), &stored).unwrap();
    chain_pem
}

#[test]
fn a_stored_certificate_is_found_by_its_names_whoever_ordered_it() {
    let state = tempfile::tempdir().unwrap();
    let dir = "https://acme.test/dir";
    let now = now_secs();
    let both = save_pair(state.path(), &["a.example.com", "b.example.com"], 90);
    let found = |names: &[&str]| Held::find(state.path(), dir, &s(names), now);
    for names in [
        &["b.example.com"][..],
        &["a.example.com"],
        &["b.example.com", "a.example.com"],
    ] {
        let h = found(names).expect("covered by the stored pair");
        assert_eq!(h.names, s(&["a.example.com", "b.example.com"]), "{names:?}");
    }
    assert!(found(&["c.example.com"]).is_none());
    let only_a = save_pair(state.path(), &["a.example.com"], 30);
    let h = found(&["a.example.com"]).unwrap();
    assert_eq!(h.names, s(&["a.example.com"]), "an exact set wins");
    assert_ne!(only_a, both);
    save_pair(state.path(), &["old.example.com"], -1);
    assert!(
        found(&["old.example.com"]).is_none(),
        "an expired one is not used"
    );
}

fn offline_plan(state: &Path, names: &[&str]) -> Plan {
    Plan {
        settings: crate::AcmeSettings {
            directory_url: "https://acme.test/dir".into(),
            ca: None,
            email: "ops@example.com".into(),
            profile: None,
            state_dir: Some(state.to_path_buf()),
            check_ports: burrow_syntax::web::CheckPorts {
                https: Some(443),
                http: None,
            },
            dns: Vec::new(),
            fallback_directory: None,
        },
        groups: [("moved".to_string(), s(names))].into_iter().collect(),
        choices: Default::default(),
    }
}

#[tokio::test]
async fn before_any_order_a_stored_certificate_that_covers_the_names_is_used() {
    let state = tempfile::tempdir().unwrap();
    save_pair(state.path(), &["a.example.com", "b.example.com"], 90);
    let acme = Arc::new(Acme::default());
    let mut worker = Worker::new(acme.clone());
    worker.adopt(&offline_plan(state.path(), &["b.example.com"]));
    let mut g = Group::new(s(&["b.example.com"]), None);
    worker.step("moved", &mut g).await;
    assert!(
        g.last_error.is_none(),
        "nothing was ordered: {:?}",
        g.last_error
    );
    assert!(acme.certificate("b.example.com").is_some());
    assert_eq!(g.view(&[]).phase, CertPhase::Ok);
}

#[tokio::test]
async fn a_certificate_due_for_renewal_is_not_reused_for_itself() {
    let state = tempfile::tempdir().unwrap();
    save_pair(state.path(), &["a.example.com"], 2);
    let acme = Arc::new(Acme::default());
    let mut worker = Worker::new(acme.clone());
    worker.adopt(&offline_plan(state.path(), &["a.example.com"]));
    let held = Held::find(
        state.path(),
        "https://acme.test/dir",
        &s(&["a.example.com"]),
        now_secs(),
    );
    assert!(held.is_some());
    let mut g = Group::new(s(&["a.example.com"]), held);
    g.planned = Some(now_secs() - 1);
    worker.step("moved", &mut g).await;
    assert!(g.last_error.is_some(), "the renewal was tried");
}

#[test]
fn a_group_waiting_for_an_order_is_in_the_queue_and_one_served_is_not() {
    let (chain, key) = pem_pair(&["a.example.com"], 90);
    let held = Held::parse(&chain, &key, &s(&["a.example.com"])).unwrap();
    let mut g = Group::new(s(&["a.example.com"]), Some(held));
    assert_eq!(g.view(&[]).queued, None, "served and not due");
    g.rename(&s(&["a.example.com", "new.example.com"]));
    let v = g.view(&[]);
    assert!(
        v.queued.is_some() && !v.running,
        "an added name waits for an order"
    );
    g.rename(&s(&["a.example.com"]));
    assert_eq!(g.view(&[]).queued, None, "a removed name orders nothing");
    let acme = Acme::default();
    acme.publish("blog", Group::new(s(&["b.example.com"]), None).view(&[]));
    acme.publish("shop", g.view(&[]));
    let q = acme.queue();
    assert_eq!(q.len(), 1);
    assert_eq!(
        (q[0].service.as_str(), q[0].names.clone()),
        ("blog", s(&["b.example.com"]))
    );
}

#[test]
fn replaces_is_sent_only_once_the_renewal_time_came_so_an_added_name_counts() {
    let (chain, key) = pem_pair(&["a.example.com"], 90);
    let mut held = Held::parse(&chain, &key, &s(&["a.example.com"])).unwrap();
    held.ari_id = Some("aki.serial".into());
    let now = now_secs();
    let mut g = Group::new(s(&["a.example.com", "b.example.com"]), Some(held));
    assert!(replaces(&g, now).is_none(), "no window known yet");
    g.planned = Some(now + 3600);
    assert!(replaces(&g, now).is_none(), "a name added mid-life");
    g.planned = Some(now);
    let id = replaces(&g, now).unwrap();
    assert_eq!(id.authority_key_identifier, "aki");
    assert_eq!(id.serial, "serial");
    g.held.as_mut().unwrap().ari_id = None;
    assert!(replaces(&g, now).is_none(), "no ARI id, no replaces");
}
