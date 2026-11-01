use crate::kit::*;

const BURST: usize = 24;

async fn burst(c: &Client) {
    let mut gets = tokio::task::JoinSet::new();
    for _ in 0..BURST {
        let mut c = c.clone();
        gets.spawn(async move { get(&mut c, &site_uri("/hold/100")).await });
    }
    for (status, _) in gets.join_all().await {
        assert_eq!(status, 200);
    }
}

#[tokio::test]
async fn bursts_of_parallel_streams_reuse_the_app_connections_of_the_first() {
    let a = app().await;
    let p = proxy(&a, |_| {}).await;
    let c = client(p.https_addr().unwrap()).await;
    burst(&c).await;
    let first = Counts::get(&a.counts.conns);
    assert!(
        first >= BURST / 2,
        "the streams overlapped: {first} app connections"
    );
    for _ in 0..3 {
        burst(&c).await;
    }
    let after = Counts::get(&a.counts.conns);
    assert!(
        after <= first + 2,
        "later bursts reuse the idle app connections: {first} then {after}"
    );
}
