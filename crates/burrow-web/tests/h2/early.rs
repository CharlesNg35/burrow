use bytes::Bytes;

use crate::kit::*;

#[tokio::test]
async fn an_early_401_resets_only_its_stream_and_the_connection_goes_on() {
    let a = app().await;
    let p = proxy(&a, |s| {
        s.sites[0].options.basic_auth = vec![burrow_web::BasicUser {
            user: "bob".into(),
            hash: bcrypt::hash("pw", 4).unwrap(),
        }];
    })
    .await;
    let mut c = client(p.https_addr().unwrap()).await;
    for _ in 0..3 {
        let req = http::Request::post(site_uri("/up")).body(()).unwrap();
        let mut ready = c.clone().ready().await.unwrap();
        let (resp, mut tx) = ready.send_request(req, false).unwrap();
        tx.send_data(Bytes::from(vec![b'x'; 4096]), false).unwrap();
        let resp = tokio::time::timeout(ANSWER, resp).await.unwrap().unwrap();
        assert_eq!(resp.status(), 401);
        let reset = std::future::poll_fn(|cx| tx.poll_reset(cx));
        let reason = tokio::time::timeout(ANSWER, reset).await.unwrap().unwrap();
        assert_eq!(reason, h2::Reason::NO_ERROR);
        drop(read_body(resp.into_body()).await);
        let auth = burrow_web::auth::basic::encode("bob", b"pw");
        let next = request(&site_uri("/next"), &[("authorization", &auth)]);
        let (status, body) = send(&mut c, next, None).await;
        assert_eq!(status, 200, "{body}");
    }
}
