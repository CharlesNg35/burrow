mod bind;
mod cloudflare;
#[allow(dead_code)]
#[path = "../acme/pebble.rs"]
mod pebble;
mod scenarios;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pebble_checks_names_over_dns_through_bind_and_cloudflare() {
    if std::env::var_os("BURROW_PEBBLE").is_none() {
        return;
    }
    let p = pebble::Pebble::start().await;
    let b = bind::Bind::start().await;
    p.run_pebble_with_dns(&["PEBBLE_AUTHZREUSE=0"], bind::ADDR)
        .await;
    let run = tokio::spawn(scenarios::all(p, b));
    let outcome = run.await;
    pebble::remove_containers().await;
    bind::remove().await;
    if let Err(e) = outcome {
        std::panic::resume_unwind(e.into_panic());
    }
}
