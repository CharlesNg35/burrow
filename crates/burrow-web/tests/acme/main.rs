#[path = "../common/mod.rs"]
mod common;
mod listener;
mod nonce;
mod pebble;
#[path = "../../../../tests/common/ports.rs"]
mod ports;
mod responder;
mod scenarios;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pebble_issues_renews_and_keeps_certificates() {
    if std::env::var_os("BURROW_PEBBLE").is_none() {
        return;
    }
    let p = pebble::Pebble::start().await;
    let run = tokio::spawn(scenarios::all(p));
    let outcome = run.await;
    pebble::remove_containers().await;
    if let Err(e) = outcome {
        std::panic::resume_unwind(e.into_panic());
    }
}
