use std::path::Path;
use std::time::{Duration, Instant};

use burrow_web::tls::acme::http::nonce_refusals;
use burrow_web::tls::acme::storage::cert_dir;
use burrow_web::tls::{CertCheck, CertPhase, Store};

use super::scenarios::{Ctx, PAST, names, open_store, served, settings, status_of};

const NAME: &str = "nonce.test";
const TRIES: usize = 60;

fn stored_pem(state: &Path) -> Option<String> {
    std::fs::read_to_string(cert_dir(state, &[NAME.to_string()]).join("cert.pem")).ok()
}

async fn despite_refusals(ctx: &Ctx, state: &Path, done: impl Fn(&Store) -> bool) -> usize {
    let s = settings(ctx, state, &[CertCheck::Tls], None);
    for attempt in 1..=TRIES {
        let store = open_store(ctx, &s, &names("nonce", &[NAME])).await;
        let start = Instant::now();
        loop {
            if done(&store) {
                return attempt;
            }
            let st = status_of(&store, NAME);
            if matches!(st.phase, CertPhase::Failing { .. }) {
                let e = st.last_error.unwrap_or_default();
                assert!(
                    e.contains("anti-replay nonce") && e.contains("(badNonce)"),
                    "only a nonce refused after instant-acme's retries may fail an order here: {e}"
                );
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(90),
                "try {attempt}: {st:#?}"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    panic!("{TRIES} orders in a row lost to refused nonces");
}

pub async fn refused_nonces_are_retried(ctx: &Ctx) {
    let before = nonce_refusals();
    let began = Instant::now();
    let state = tempfile::tempdir().unwrap();
    let state = state.path();
    let ok = |store: &Store| status_of(store, NAME).phase == CertPhase::Ok;
    let issued = despite_refusals(ctx, state, ok).await;
    let old = stored_pem(state).expect("the certificate is stored");
    let first = served(ctx, NAME).await.expect("served");
    ctx.p.set_renewal_info(&old, PAST).await;
    let renewed = despite_refusals(ctx, state, |store| {
        ok(store) && stored_pem(state).is_some_and(|p| p != old)
    })
    .await;
    let now = served(ctx, NAME).await.expect("served after the renewal");
    assert_ne!(now, first, "the renewed certificate is served");
    let refused = nonce_refusals() - before;
    println!(
        "Pebble refused {refused} nonces; the certificate came on order {issued}, the renewal on {renewed}, in {} s",
        began.elapsed().as_secs()
    );
    assert!(
        refused > 0,
        "with PEBBLE_WFE_NONCEREJECT=50 Pebble refuses about every second nonce"
    );
}
