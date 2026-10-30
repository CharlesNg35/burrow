mod acme_dns;
mod bind;
mod cloudflare;
#[allow(dead_code)]
#[path = "../acme/pebble.rs"]
mod pebble;
#[allow(dead_code)]
#[path = "../../../../tests/common/ports.rs"]
mod ports;
mod scenarios;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pebble_checks_names_over_dns_through_bind_and_cloudflare() {
    if std::env::var_os("BURROW_PEBBLE").is_none() {
        return;
    }
    let p = pebble::Pebble::start().await;
    let started = match acme_dns::AcmeDns::start().await {
        Ok(a) => bind::Bind::start(&a.records()).await.map(|b| (a, b)),
        Err(why) => Err(why),
    };
    let (a, b) = match started {
        Ok(both) => both,
        Err(why) => {
            remove_all().await;
            panic!("{why}");
        }
    };
    p.run_pebble_with_dns(&["PEBBLE_AUTHZREUSE=0"], bind::ADDR)
        .await;
    let run = tokio::spawn(scenarios::all(p, b, a));
    let outcome = run.await;
    remove_all().await;
    if let Err(e) = outcome {
        std::panic::resume_unwind(e.into_panic());
    }
}

async fn remove_all() {
    pebble::remove_containers().await;
    bind::remove().await;
    acme_dns::remove().await;
}
