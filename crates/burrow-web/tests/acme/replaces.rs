use std::path::Path;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use burrow_web::tls::acme::storage;
use burrow_web::tls::{CertCheck, CertPhase, CertState, Certificates, Store};

use super::pebble::DIRECTORY;
use super::scenarios::{Ctx, names, orders, served, settings, stored_leaf, until};

const NAME: &str = "replaced.test";

fn pebble_aki(issued: &Path) -> Vec<u8> {
    let facts = burrow_web::tls::x509::facts(&stored_leaf(issued)).unwrap();
    let id = facts
        .ari_id
        .expect("Pebble's certificate names its issuer's key");
    let (aki, _) = id.split_once('.').unwrap();
    URL_SAFE_NO_PAD.decode(aki).unwrap()
}

fn unknown_to_pebble_with_the_top_bit_set(aki: Vec<u8>) -> (String, String) {
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let mut ca = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca.key_identifier_method = rcgen::KeyIdMethod::PreSpecified(aki);
    let issuer = rcgen::Issuer::new(ca, ca_key);
    let key = rcgen::KeyPair::generate().unwrap();
    let mut leaf = rcgen::CertificateParams::new(vec![NAME.to_string()]).unwrap();
    leaf.use_authority_key_identifier_extension = true;
    leaf.serial_number = Some(rcgen::SerialNumber::from_slice(&[
        0x86, 0x4D, 0x3F, 0x5B, 0x12, 0x14, 0x13,
    ]));
    let now = time::OffsetDateTime::now_utc();
    leaf.not_before = now - time::Duration::days(89);
    leaf.not_after = now + time::Duration::days(1);
    let cert = leaf.signed_by(&key, &issuer).unwrap();
    (cert.pem(), key.serialize_pem())
}

pub(super) async fn a_refused_replaces_order_is_followed_by_a_plain_order(
    ctx: &Ctx,
    issued: &Path,
) {
    let state = tempfile::tempdir().unwrap();
    let (chain_pem, key_pem) = unknown_to_pebble_with_the_top_bit_set(pebble_aki(issued));
    let name = vec![NAME.to_string()];
    let stored = storage::Stored {
        chain_pem,
        key_pem,
        meta: storage::Meta {
            directory: DIRECTORY.into(),
            names: name.clone(),
            issued: 1,
        },
    };
    storage::save(&storage::cert_dir(state.path(), &name), &stored).unwrap();
    let s = settings(ctx, state.path(), &[CertCheck::Tls], None);
    let store = Store::new();
    store.set_acme_backoff(Duration::from_secs(3));
    ctx.r.serve(store.clone());
    store.apply(&s, &names("renew", &[NAME])).await;
    let st = until(&store, NAME, "the second refused replaces order", |s| {
        s.last_error
            .as_deref()
            .is_some_and(|e| e.contains("without replacing it"))
    })
    .await;
    let e = st.last_error.clone().unwrap();
    assert!(e.contains("serverInternal"), "Pebble's own answer: {e}");
    assert_eq!(st.failures, 2, "{st:#?}");
    assert_eq!(orders(state.path()), 0, "{st:#?}");
    assert_eq!(
        store.state(NAME),
        CertState::Ready,
        "the old one serves meanwhile"
    );
    let st = until(&store, NAME, "the plain order on the third try", |s| {
        s.phase == CertPhase::Ok
    })
    .await;
    assert!(st.last_error.is_none(), "{st:#?}");
    assert_eq!(
        st.failures, 2,
        "two refusals, then the plain order: {st:#?}"
    );
    assert!(served(ctx, NAME).await.is_some(), "the new one is served");
    let list = burrow_web::tls::acme::guard::load(&burrow_web::tls::acme::guard::path(
        state.path(),
        DIRECTORY,
    ));
    assert_eq!(list.len(), 1, "one order");
    assert!(
        !list[0].ari,
        "ordered without replaces, so the guard counts it"
    );
}
