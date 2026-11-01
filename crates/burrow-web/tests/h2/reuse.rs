use std::time::Duration;

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

#[tokio::test]
async fn slow_readers_on_kept_h2_app_connections_do_not_stall_another_visitor() {
    let a = app_h2c().await;
    let p = proxy(&a, |s| {
        for site in &mut s.sites {
            site.options.target_http2 = true;
        }
    })
    .await;
    let addr = p.https_addr().unwrap();
    let mut stalled = Vec::new();
    for _ in 0..2 {
        let c = client(addr).await;
        let mut ready = c.clone().ready().await.unwrap();
        let (resp, _) = ready
            .send_request(request(&site_uri("/big/4194304"), &[]), true)
            .unwrap();
        let resp = resp.await.unwrap();
        assert_eq!(resp.status(), 200);
        stalled.push((c, resp.into_body()));
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut third = client(addr).await;
    let (status, _) = tokio::time::timeout(Duration::from_secs(5), get(&mut third, &site_uri("/")))
        .await
        .expect("a third visitor is not stalled by the slow readers");
    assert_eq!(status, 200);
    assert!(
        Counts::get(&a.counts.conns) >= 2,
        "each slow reader holds its own app connection"
    );
}
