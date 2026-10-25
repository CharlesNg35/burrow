#[path = "../common/mod.rs"]
mod common;
mod interop;
#[allow(dead_code)]
#[path = "../h2/kit.rs"]
mod kit;

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use burrow_web::{Proxy, Site, TargetTls};
use bytes::Bytes;
use common::*;
use interop::*;
use kit::{H1, H2, tls_with};

const PLAIN: &str = "grpc.example.com";
const SECURE: &str = "grpcs.example.com";

fn grpc_site(service: &str, host: &str, tls: bool, wait: Duration) -> Site {
    let mut s = https_site(service, &[host]);
    s.options.target_http2 = true;
    s.options.response_header_timeout = wait;
    if tls {
        s.options.target_tls = TargetTls::Unchecked;
        s.options.target_name = TLS_NAME.into();
    }
    s
}

async fn proxy(wait: Duration) -> Proxy<Fake> {
    let (up, _) = Fake::new(4);
    up.serve("h2c", H2C.parse().unwrap());
    up.serve("tls", TLS.parse().unwrap());
    start_with(
        |s| {
            s.sites = vec![
                grpc_site("h2c", PLAIN, false, wait),
                grpc_site("tls", SECURE, true, wait),
            ];
        },
        up,
        fixed(&[PLAIN, SECURE]),
        false,
    )
    .await
}

type Client = h2::client::SendRequest<Bytes>;

async fn client(addr: SocketAddr, host: &str) -> Client {
    let t = tls_with(addr, host, &[H2]).await.unwrap();
    let (c, conn) = h2::client::handshake(t).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    c.ready().await.unwrap()
}

fn grpc(host: &str, method: &str, extra: &[(&str, &str)]) -> http::Request<()> {
    let mut b = http::Request::post(format!("https://{host}/grpc.testing.TestService/{method}"))
        .header("content-type", "application/grpc")
        .header("te", "trailers");
    for (k, v) in extra {
        b = b.header(*k, *v);
    }
    b.body(()).unwrap()
}

struct Answer {
    status: u16,
    headers: http::HeaderMap,
    trailers: http::HeaderMap,
    replies: Vec<Vec<u8>>,
}

impl Answer {
    fn grpc_status(&self) -> Option<&str> {
        self.trailers
            .get("grpc-status")
            .or_else(|| self.headers.get("grpc-status"))
            .and_then(|v| v.to_str().ok())
    }
}

fn grpc_answer(what: &str, status: u16, headers: &http::HeaderMap, data: &[u8]) -> Vec<Vec<u8>> {
    let kind = headers.get("content-type").and_then(|v| v.to_str().ok());
    assert!(
        kind.is_some_and(|k| k.starts_with("application/grpc")),
        "{what}: not a gRPC answer: status {status}, content-type {kind:?}, body starts {:?}",
        head_of(data)
    );
    messages(data).unwrap_or_else(|e| panic!("{what}: status {status}: {e}"))
}

fn frames(what: &str, data: &[u8]) -> Vec<Vec<u8>> {
    messages(data).unwrap_or_else(|e| panic!("{what}: {e}"))
}

async fn finish(what: &str, resp: h2::client::ResponseFuture) -> Answer {
    let resp = tokio::time::timeout(Duration::from_secs(20), resp)
        .await
        .expect("an answer in time")
        .unwrap();
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let mut body = resp.into_body();
    let mut data = Vec::new();
    while let Some(chunk) = body.data().await {
        let chunk = chunk.unwrap();
        let _ = body.flow_control().release_capacity(chunk.len());
        data.extend_from_slice(&chunk);
    }
    let trailers = body.trailers().await.unwrap().unwrap_or_default();
    let replies = grpc_answer(what, status, &headers, &data);
    Answer {
        status,
        headers,
        trailers,
        replies,
    }
}

async fn unary(
    c: &mut Client,
    host: &str,
    method: &str,
    extra: &[(&str, &str)],
    proto: &[u8],
) -> Answer {
    let mut c = c.clone().ready().await.unwrap();
    let (resp, mut tx) = c.send_request(grpc(host, method, extra), false).unwrap();
    tx.send_data(Bytes::from(message(proto)), true).unwrap();
    finish(&format!("{host} {method}"), resp).await
}

async fn calls_work(addr: SocketAddr, host: &str) {
    let mut c = client(addr, host).await;
    let a = unary(&mut c, host, "EmptyCall", &[], &[]).await;
    assert_eq!(
        (a.status, a.grpc_status()),
        (200, Some("0")),
        "{host} EmptyCall"
    );
    assert_eq!(a.replies, [Vec::<u8>::new()]);

    let echo = [
        ("x-grpc-test-echo-initial", "first"),
        ("x-grpc-test-echo-trailing-bin", "AAEC"),
    ];
    let a = unary(&mut c, host, "UnaryCall", &echo, &number(2, 10)).await;
    assert_eq!(a.grpc_status(), Some("0"), "{host} UnaryCall");
    assert_eq!(
        a.headers["x-grpc-test-echo-initial"], "first",
        "metadata both ways"
    );
    assert_eq!(
        a.trailers["x-grpc-test-echo-trailing-bin"], "AAEC",
        "trailers back"
    );

    let a = unary(&mut c, host, "UnaryCall", &[], &status_request(7, "no")).await;
    assert_eq!(
        a.grpc_status(),
        Some("7"),
        "{host}: the app's error status passes"
    );

    let mut s = c.clone().ready().await.unwrap();
    let (resp, mut tx) = s
        .send_request(grpc(host, "StreamingInputCall", &[]), false)
        .unwrap();
    for n in [10, 20, 30] {
        tx.send_data(Bytes::from(message(&input_request(n))), false)
            .unwrap();
    }
    tx.send_data(Bytes::new(), true).unwrap();
    let a = finish(&format!("{host} StreamingInputCall"), resp).await;
    assert_eq!(a.grpc_status(), Some("0"));
    assert_eq!(a.replies, [number(1, 60)], "{host}: client stream");
}

async fn streams_flow(addr: SocketAddr, host: &str) {
    let c = client(addr, host).await;
    let started = Instant::now();
    let mut s = c.clone().ready().await.unwrap();
    let (resp, mut tx) = s
        .send_request(grpc(host, "StreamingOutputCall", &[]), false)
        .unwrap();
    tx.send_data(
        Bytes::from(message(&output_request(&[1, 2, 3], 300_000))),
        true,
    )
    .unwrap();
    let resp = resp.await.unwrap();
    let what = format!("{host} StreamingOutputCall");
    grpc_answer(&what, resp.status().as_u16(), resp.headers(), &[]);
    let mut body = resp.into_body();
    let first = body.data().await.unwrap().unwrap();
    let _ = body.flow_control().release_capacity(first.len());
    assert!(
        started.elapsed() < Duration::from_millis(800),
        "{host}: the first message arrives before the last is sent: {:?}",
        started.elapsed()
    );
    while let Some(chunk) = body.data().await {
        let _ = body.flow_control().release_capacity(chunk.unwrap().len());
    }
    let trailers = body.trailers().await.unwrap().unwrap();
    assert_eq!(trailers["grpc-status"], "0");

    let mut s = c.clone().ready().await.unwrap();
    let (resp, mut tx) = s
        .send_request(grpc(host, "FullDuplexCall", &[]), false)
        .unwrap();
    tx.send_data(Bytes::from(message(&output_request(&[4], 0))), false)
        .unwrap();
    let resp = resp.await.unwrap();
    let what = format!("{host} FullDuplexCall");
    grpc_answer(&what, resp.status().as_u16(), resp.headers(), &[]);
    let mut body = resp.into_body();
    for round in 0..3 {
        let got = body.data().await.unwrap().unwrap();
        let _ = body.flow_control().release_capacity(got.len());
        assert_eq!(
            frames(&what, &got).len(),
            1,
            "{host}: reply {round} before the next send"
        );
        tx.send_data(Bytes::from(message(&output_request(&[4], 0))), round == 2)
            .unwrap();
    }
    while let Some(chunk) = body.data().await {
        let _ = body.flow_control().release_capacity(chunk.unwrap().len());
    }
    assert_eq!(body.trailers().await.unwrap().unwrap()["grpc-status"], "0");
}

async fn deadlines_pass(addr: SocketAddr, host: &str) {
    let c = client(addr, host).await;
    let mut s = c.clone().ready().await.unwrap();
    let req = grpc(host, "FullDuplexCall", &[("grpc-timeout", "300m")]);
    let started = Instant::now();
    let (resp, _tx) = s.send_request(req, false).unwrap();
    let ended = tokio::time::timeout(Duration::from_secs(10), resp)
        .await
        .expect("the app ends the call at its deadline");
    let took = started.elapsed();
    match ended {
        Err(e) => assert_eq!(
            e.reason(),
            Some(h2::Reason::CANCEL),
            "{host}: the app's reset at the deadline reaches the visitor as is: {e:?}"
        ),
        Ok(resp) => {
            let status = resp.status().as_u16();
            let headers = resp.headers().clone();
            assert!(
                headers.get("grpc-status").is_some_and(|v| v == "4"),
                "{host}: DEADLINE_EXCEEDED, got status {status} {headers:?}"
            );
        }
    }
    assert!(
        took >= Duration::from_millis(250) && took < Duration::from_secs(5),
        "{host}: grpc-timeout reached the app, which ended the call after {took:?}"
    );
}

async fn http1_visitor_is_answered(addr: SocketAddr, host: &str) {
    let mut t = tls_with(addr, host, &[H1]).await.unwrap();
    let r = exchange(
        &mut t,
        format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes(),
    )
    .await;
    assert!(r.starts_with("HTTP/1.1 "), "{r}");
    assert!(
        !r.starts_with("HTTP/1.1 502"),
        "{host}: an HTTP/1.1 visitor reaches the h2 app: {r}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_through_burrow_to_h2c_and_to_h2_over_target_tls() {
    if std::env::var_os("BURROW_GRPC").is_none() {
        return;
    }
    if let Err(e) = interop::start().await {
        interop::remove().await;
        panic!("{e}");
    }
    let run = tokio::spawn(async {
        let p = proxy(Duration::from_secs(60)).await;
        let addr = p.https_addr().unwrap();
        for host in [PLAIN, SECURE] {
            calls_work(addr, host).await;
            streams_flow(addr, host).await;
            deadlines_pass(addr, host).await;
            http1_visitor_is_answered(addr, host).await;
        }
    })
    .await;
    interop::remove().await;
    if let Err(e) = run {
        std::panic::resume_unwind(e.into_panic());
    }
}
