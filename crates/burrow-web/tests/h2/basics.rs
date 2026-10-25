use std::time::Duration;

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use tokio_websockets::{ClientBuilder, Message};

use crate::common::*;
use crate::kit::*;

#[tokio::test]
async fn alpn_offers_h2_then_http1_and_web_http2_false_offers_http1_only() {
    let a = app().await;
    let on = proxy(&a, |_| {}).await;
    let addr = on.https_addr().unwrap();
    let t = tls_with(addr, HOST, &[H2, H1]).await.unwrap();
    assert_eq!(alpn_of(&t).as_deref(), Some(H2));
    let t = tls_with(addr, HOST, &[H1]).await.unwrap();
    assert_eq!(alpn_of(&t).as_deref(), Some(H1));
    let t = tls_with(addr, HOST, &[]).await.unwrap();
    assert_eq!(alpn_of(&t), None, "no ALPN still means HTTP/1.1");
    let off = proxy(&a, |s| s.http2 = false).await;
    let addr = off.https_addr().unwrap();
    let t = tls_with(addr, HOST, &[H2, H1]).await.unwrap();
    assert_eq!(alpn_of(&t).as_deref(), Some(H1));
    assert!(
        tls_with(addr, HOST, &[H2]).await.is_err(),
        "a client offering only h2 fails the handshake"
    );
}

#[tokio::test]
async fn reload_turns_http2_off_for_new_connections() {
    let a = app().await;
    let p = proxy(&a, |_| {}).await;
    let addr = p.https_addr().unwrap();
    let mut s = burrow_web::ProxySettings::new(addr);
    s.sites = vec![https_site("site", &[HOST]), https_site("other", &[OTHER])];
    s.http2 = false;
    p.apply(&s).await;
    let t = tls_with(addr, HOST, &[H2, H1]).await.unwrap();
    assert_eq!(alpn_of(&t).as_deref(), Some(H1));
}

#[tokio::test]
async fn an_h2_request_reaches_the_app_over_http1_with_host_from_the_authority() {
    let a = app().await;
    let p = proxy(&a, |_| {}).await;
    let mut c = client(p.https_addr().unwrap()).await;
    let (status, body) = get(&mut c, &site_uri("/x?y=1")).await;
    assert_eq!(status, 200);
    assert!(body.starts_with("GET /x?y=1 HTTP/1.1\n"), "{body}");
    assert!(body.contains(&format!("\nhost: {HOST}\n")), "{body}");
    assert!(body.contains("\nx-forwarded-proto: https\n"), "{body}");
}

#[tokio::test]
async fn another_name_on_a_reused_connection_gets_421_and_a_different_host_gets_400() {
    let a = app().await;
    let p = proxy(&a, |_| {}).await;
    let mut c = client(p.https_addr().unwrap()).await;
    let (status, _) = get(&mut c, &format!("https://{OTHER}/")).await;
    assert_eq!(
        status, 421,
        "the certificate covers both, the connection is for one"
    );
    let req = request(&site_uri("/"), &[("host", OTHER)]);
    let (status, _) = send(&mut c, req, None).await;
    assert_eq!(status, 400, "RFC 9113 8.3.1");
    let req = request(&site_uri("/"), &[("host", HOST)]);
    assert_eq!(send(&mut c, req, None).await.0, 200);
    assert_eq!(Counts::get(&a.counts.requests), 1);
}

fn fields(n: usize) -> Vec<(String, String)> {
    (0..n)
        .map(|i| (format!("x-f{i}"), "1".to_string()))
        .collect()
}

fn with_fields(f: &[(String, String)]) -> http::Request<()> {
    let mut req = request(&site_uri("/"), &[]);
    for (k, v) in f {
        req.headers_mut().append(
            http::HeaderName::try_from(k.as_str()).unwrap(),
            v.parse().unwrap(),
        );
    }
    req
}

#[tokio::test]
async fn one_hundred_fields_pass_and_a_hundred_and_first_or_a_cookie_crumb_gets_431() {
    let a = app().await;
    let p = proxy(&a, |_| {}).await;
    let mut c = client(p.https_addr().unwrap()).await;
    assert_eq!(send(&mut c, with_fields(&fields(100)), None).await.0, 200);
    assert_eq!(send(&mut c, with_fields(&fields(101)), None).await.0, 431);
    let mut f = fields(99);
    f.push(("cookie".into(), "a=1".into()));
    f.push(("cookie".into(), "b=2".into()));
    assert_eq!(
        send(&mut c, with_fields(&f), None).await.0,
        431,
        "each cookie crumb is a field"
    );
    assert_eq!(Counts::get(&a.counts.requests), 1);
}

#[tokio::test]
async fn cookie_crumbs_reach_an_http1_app_joined() {
    let a = app().await;
    let p = proxy(&a, |_| {}).await;
    let mut c = client(p.https_addr().unwrap()).await;
    let req = request(&site_uri("/"), &[("cookie", "a=1"), ("cookie", "b=2")]);
    let (_, body) = send(&mut c, req, None).await;
    assert!(body.contains("\ncookie: a=1; b=2\n"), "{body}");
    assert_eq!(body.matches("cookie:").count(), 1, "{body}");
}

#[tokio::test]
async fn an_8_mib_upload_and_300_parallel_streams_finish_on_one_connection() {
    let a = app().await;
    let p = proxy(&a, |_| {}).await;
    let mut c = client(p.https_addr().unwrap()).await;
    let big = Bytes::from(vec![b'u'; 8 * 1024 * 1024]);
    let mut post = request(&site_uri("/up"), &[]);
    *post.method_mut() = http::Method::POST;
    let (status, body) = send(&mut c, post, Some(big)).await;
    assert_eq!(status, 200);
    assert!(body.contains("body-len: 8388608"), "{body}");
    assert_eq!(c.current_max_send_streams(), 100, "max_concurrent_streams");
    let mut tasks = Vec::new();
    for i in 0..300 {
        let mut c = c.clone();
        tasks.push(tokio::spawn(async move {
            get(&mut c, &site_uri(&format!("/hold/30?n={i}"))).await
        }));
    }
    for (i, t) in tasks.into_iter().enumerate() {
        let (status, body) = t.await.unwrap();
        assert_eq!(status, 200);
        assert!(body.starts_with(&format!("GET /hold/30?n={i} ")), "{body}");
    }
    let most = Counts::get(&a.counts.most_live);
    assert!(
        most <= burrow_web::http::upstream::pool::APP_CONNECTIONS,
        "{most} app connections at once for one visitor connection"
    );
}

#[tokio::test]
async fn a_quiet_open_stream_outlives_the_idle_time() {
    let a = app().await;
    let p = proxy(&a, |s| s.timeouts.idle = Duration::from_secs(1)).await;
    let mut c = client(p.https_addr().unwrap()).await;
    let started = tokio::time::Instant::now();
    let (status, body) = get(&mut c, &site_uri("/quiet/2500")).await;
    assert_eq!(
        (status, body.as_str()),
        (200, "zzzz"),
        "no bytes for 2.5 s, idle is 1 s"
    );
    assert!(started.elapsed() >= Duration::from_millis(2500));
}

#[tokio::test]
async fn an_idle_h2_connection_closes_although_the_client_keeps_pinging() {
    let a = app().await;
    let p = proxy(&a, |s| s.timeouts.idle = Duration::from_secs(1)).await;
    let Raw { mut rd, mut wr } = raw(p.https_addr().unwrap()).await;
    let started = tokio::time::Instant::now();
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        while wr.write_all(&frame(PING, 0, 0, &[0; 8])).await.is_ok() {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });
    let mut pongs = 0;
    while let Some(f) = read_frame(&mut rd, Duration::from_secs(5)).await {
        if f.ty == PING && f.flags & ACK == ACK {
            pongs += 1;
        }
    }
    assert!(pongs >= 3, "PINGs are answered ({pongs})");
    let took = started.elapsed();
    assert!(
        took >= Duration::from_secs(1) && took < Duration::from_secs(4),
        "closed after the idle time although the client kept pinging: {took:?}"
    );
}

#[tokio::test]
async fn shutdown_sends_goaway_and_lets_the_open_request_finish() {
    let a = app().await;
    let p = proxy(&a, |_| {}).await;
    let mut c = client(p.https_addr().unwrap()).await;
    let held = tokio::spawn(async move { get(&mut c, &site_uri("/hold/500")).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    p.stop().await;
    let (status, _) = held.await.unwrap();
    assert_eq!(status, 200);
}

#[tokio::test]
async fn a_websocket_uses_its_own_http1_connection_beside_h2() {
    let a = app().await;
    let ws = crate::ws_echo().await;
    let (up, _) = Fake::new(4);
    up.serve("site", a.addr);
    up.serve("chat", ws);
    let p = start_with(
        |s| s.sites = vec![https_site("site", &[HOST]), https_site("chat", &[OTHER])],
        up,
        fixed(&[HOST, OTHER]),
        false,
    )
    .await;
    let addr = p.https_addr().unwrap();
    let t = tls_with(addr, OTHER, &[H2, H1]).await.unwrap();
    let (c, conn) = h2::client::handshake(t).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let c = c.ready().await.unwrap();
    assert!(
        !c.is_extended_connect_protocol_enabled(),
        "no RFC 8441 in the SETTINGS"
    );
    let t = tls_with(addr, OTHER, &[H1]).await.unwrap();
    let (mut ws, answer) = ClientBuilder::new()
        .uri(&format!("wss://{OTHER}/socket"))
        .unwrap()
        .connect_on(t)
        .await
        .unwrap();
    assert_eq!(answer.status(), 101);
    ws.send(Message::text("hi")).await.unwrap();
    let got = ws.next().await.unwrap().unwrap();
    assert_eq!(got.as_text(), Some("hi"));
}
