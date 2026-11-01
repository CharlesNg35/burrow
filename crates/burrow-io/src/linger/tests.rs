use std::convert::Infallible;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::Full;
use hyper::{Request, Response, StatusCode};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::net::TcpListener;
use tokio::time::Instant;

use super::*;

async fn refuse(_: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
    let mut r = Response::new(Full::new(Bytes::from_static(b"no\n")));
    *r.status_mut() = StatusCode::UNAUTHORIZED;
    Ok(r)
}

async fn early_answer_to_a_large_body(keep_alive: bool, upgrades: bool) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut b = hyper::server::conn::http1::Builder::new();
        b.keep_alive(keep_alive);
        let conn = b.serve_connection(TokioIo::new(stream), hyper::service::service_fn(refuse));
        if upgrades {
            serve(conn.with_upgrades()).await.unwrap();
        } else {
            serve(conn).await.unwrap();
        }
    });
    let head = format!(
        "POST / HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n",
        crate::linger::probe::BODY
    );
    let ended = crate::linger::probe::send_large_body(addr, &head).await;
    assert!(ended.answer.starts_with("HTTP/1.1 401"), "{ended:?}");
    assert!(ended.broken.is_none(), "{ended:?}");
}

#[tokio::test]
async fn an_early_answer_to_a_large_body_ends_without_a_reset() {
    early_answer_to_a_large_body(false, false).await;
}

#[tokio::test]
async fn an_early_answer_on_a_keep_alive_upgradeable_connection_ends_without_a_reset() {
    early_answer_to_a_large_body(true, true).await;
}

fn pair() -> (DuplexStream, DuplexStream) {
    tokio::io::duplex(64 * 1024)
}

#[tokio::test(start_paused = true)]
async fn lingering_shuts_the_write_side_and_ends_when_the_client_closes() {
    let (server, mut client) = pair();
    let started = Instant::now();
    let lingering = tokio::spawn(linger(server));
    client.write_all(b"rest of the body").await.unwrap();
    let mut seen = Vec::new();
    client.read_to_end(&mut seen).await.unwrap();
    assert!(seen.is_empty());
    drop(client);
    lingering.await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[tokio::test(start_paused = true)]
async fn lingering_ends_after_the_idle_timeout_when_nothing_more_comes() {
    let (server, client) = pair();
    let started = Instant::now();
    linger(server).await;
    let took = started.elapsed();
    assert!(
        took >= LINGER_TIMEOUT && took < LINGER_TIMEOUT + Duration::from_secs(1),
        "{took:?}"
    );
    drop(client);
}

#[tokio::test(start_paused = true)]
async fn lingering_ends_after_the_time_cap_however_slowly_data_trickles() {
    let (server, mut client) = pair();
    let trickle = tokio::spawn(async move {
        while client.write_all(b"x").await.is_ok() {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    let started = Instant::now();
    linger(server).await;
    let took = started.elapsed();
    assert!(
        took >= LINGER_TIME && took < LINGER_TIME + Duration::from_secs(1),
        "{took:?}"
    );
    trickle.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn lingering_reads_no_more_than_the_byte_cap() {
    let (server, mut client) = pair();
    let flood = tokio::spawn(async move {
        let chunk = [b'x'; 4096];
        let mut sent = 0u64;
        while client.write_all(&chunk).await.is_ok() {
            sent += chunk.len() as u64;
        }
        sent
    });
    let started = Instant::now();
    linger(server).await;
    assert!(started.elapsed() < Duration::from_secs(1));
    let sent = flood.await.unwrap();
    assert!(
        (LINGER_BYTES..=LINGER_BYTES + 128 * 1024).contains(&sent),
        "{sent}"
    );
}
