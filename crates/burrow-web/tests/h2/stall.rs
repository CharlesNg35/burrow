use std::time::Duration;

use burrow_web::http::STALL;
use tokio::time::{Instant, sleep, timeout};

use crate::kit::*;

const BIG: usize = 8 * 1024 * 1024;
const TAKEN: usize = 128 * 1024;

async fn open(c: &Client, path: &str) -> h2::RecvStream {
    let mut c = c.clone().ready().await.unwrap();
    let (resp, _) = c.send_request(request(&site_uri(path), &[]), true).unwrap();
    let resp = timeout(Duration::from_secs(10), resp)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resp.status(), 200);
    resp.into_body()
}

async fn take(body: &mut h2::RecvStream, at_least: usize) -> usize {
    let mut got = 0;
    while got < at_least {
        let chunk = body.data().await.expect("more data").expect("no reset yet");
        let _ = body.flow_control().release_capacity(chunk.len());
        got += chunk.len();
    }
    got
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_visitor_that_stops_reading_loses_the_stream_after_the_stall_time_and_the_app_body_is_dropped()
 {
    if std::env::var_os("BURROW_H2_SLOW").is_none() {
        return;
    }
    let a = app().await;
    let p = proxy(&a, |_| {}).await;
    let c = client(p.https_addr().unwrap()).await;
    let _quiet = open(&c, "/quiet/60000").await;
    let mut big = open(&c, &format!("/big/{BIG}")).await;
    let mut got = take(&mut big, TAKEN).await;
    let stopped = Instant::now();
    let before = STALL - Duration::from_secs(1);
    sleep(before).await;
    assert_eq!(
        Counts::get(&a.counts.cut),
        0,
        "the app still streams {before:?} after the visitor stopped reading"
    );
    while Counts::get(&a.counts.cut) == 0 {
        assert!(
            stopped.elapsed() < STALL + Duration::from_secs(5),
            "the app's body is dropped soon after {STALL:?}"
        );
        sleep(Duration::from_millis(100)).await;
    }
    let mut ended = None;
    while ended.is_none() {
        match timeout(Duration::from_secs(5), big.data()).await {
            Ok(Some(Ok(chunk))) => {
                let _ = big.flow_control().release_capacity(chunk.len());
                got += chunk.len();
            }
            Ok(Some(Err(e))) => ended = Some(e),
            Ok(None) => panic!("the stalled stream ended cleanly after {got} bytes"),
            Err(_) => panic!("the stalled stream is still open after {got} bytes"),
        }
    }
    assert!(
        ended.unwrap().is_reset(),
        "the visitor sees the stream reset"
    );
    assert!(got < BIG, "only buffered bytes arrive: {got}");
    let mut c = c.clone();
    let (status, _) = get(&mut c, &site_uri("/after")).await;
    assert_eq!(status, 200, "the connection and the quiet stream stay open");
}
