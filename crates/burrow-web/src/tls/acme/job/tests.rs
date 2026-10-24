use super::*;

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
    storage::save(&storage::group_dir(state.path(), "blog"), &stored).unwrap();
    assert!(Held::stored(state.path(), "blog", "https://acme.test/dir").is_some());
    assert!(Held::stored(state.path(), "blog", super::super::LETSENCRYPT_STAGING).is_none());
    assert!(Held::stored(state.path(), "shop", "https://acme.test/dir").is_none());
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
