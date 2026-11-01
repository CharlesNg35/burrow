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
        let conn = b.serve_connection(
            TokioIo::new(ShutOnce::new(stream)),
            hyper::service::service_fn(refuse),
        );
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
    let lingering = tokio::spawn(linger(ShutOnce::new(server)));
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
    linger(ShutOnce::new(server)).await;
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
    linger(ShutOnce::new(server)).await;
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
    linger(ShutOnce::new(server)).await;
    assert!(started.elapsed() < Duration::from_secs(1));
    let sent = flood.await.unwrap();
    assert!(
        (LINGER_BYTES..=LINGER_BYTES + 128 * 1024).contains(&sent),
        "{sent}"
    );
}

async fn hyper_answer_to(head: String) -> crate::linger::probe::Ended {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut b = hyper::server::conn::http1::Builder::new();
        b.max_buf_size(8192);
        let conn = b.serve_connection(
            TokioIo::new(ShutOnce::new(stream)),
            hyper::service::service_fn(refuse),
        );
        serve(conn.with_upgrades()).await
    });
    let ended = crate::linger::probe::send_large_body(addr, &head).await;
    let served = server.await.unwrap();
    assert!(
        served.is_err_and(|e| e.is_parse()),
        "the parse error still reaches the caller"
    );
    ended
}

#[tokio::test]
async fn hypers_own_431_reaches_the_client_intact_when_more_bytes_follow() {
    let head = format!(
        "GET / HTTP/1.1\r\nHost: x\r\nX-Big: {}",
        "a".repeat(16 * 1024)
    );
    let ended = hyper_answer_to(head).await;
    assert!(ended.answer.starts_with("HTTP/1.1 431"), "{ended:?}");
    assert!(ended.broken.is_none(), "{ended:?}");
}

#[tokio::test]
async fn hypers_own_400_reaches_the_client_intact_when_more_bytes_follow() {
    let ended = hyper_answer_to("G@T / HTTP/1.1\r\nHost: x\r\n\r\n".into()).await;
    assert!(ended.answer.starts_with("HTTP/1.1 400"), "{ended:?}");
    assert!(ended.broken.is_none(), "{ended:?}");
}

#[tokio::test]
async fn a_head_that_times_out_closes_at_once_without_lingering() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut b = hyper::server::conn::http1::Builder::new();
        b.timer(hyper_util::rt::TokioTimer::new())
            .header_read_timeout(Duration::from_millis(200));
        let conn = b.serve_connection(
            TokioIo::new(ShutOnce::new(stream)),
            hyper::service::service_fn(refuse),
        );
        let started = Instant::now();
        let served = serve(conn).await;
        (served, started.elapsed())
    });
    let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
    client.write_all(b"GET / HTTP/1.1\r\nHo").await.unwrap();
    let (served, took) = server.await.unwrap();
    assert!(served.is_err_and(|e| e.is_timeout()));
    assert!(took < LINGER_TIMEOUT, "{took:?}");
    drop(client);
}

struct Counted {
    io: DuplexStream,
    shutdowns: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl Counted {
    fn new(io: DuplexStream) -> (Self, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        let shutdowns = std::sync::Arc::default();
        let counted = Counted {
            io,
            shutdowns: std::sync::Arc::clone(&shutdowns),
        };
        (counted, shutdowns)
    }
}

impl AsyncRead for Counted {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}

impl AsyncWrite for Counted {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let shut = ready!(Pin::new(&mut self.io).poll_shutdown(cx));
        self.shutdowns
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Poll::Ready(shut)
    }
}

async fn shutdowns_after_an_early_answer(upgrades: bool) -> usize {
    let (server, mut client) = pair();
    let (server, shutdowns) = Counted::new(server);
    let served = tokio::spawn(async move {
        let mut b = hyper::server::conn::http1::Builder::new();
        b.keep_alive(false);
        let conn = b.serve_connection(
            TokioIo::new(ShutOnce::new(server)),
            hyper::service::service_fn(refuse),
        );
        if upgrades {
            serve(conn.with_upgrades()).await.unwrap();
        } else {
            serve(conn).await.unwrap();
        }
    });
    client
        .write_all(b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 100000\r\n\r\nsome")
        .await
        .unwrap();
    let mut seen = Vec::new();
    client.read_to_end(&mut seen).await.unwrap();
    assert!(seen.starts_with(b"HTTP/1.1 401"));
    client.write_all(b"more of the body").await.unwrap();
    drop(client);
    served.await.unwrap();
    shutdowns.load(std::sync::atomic::Ordering::Relaxed)
}

#[tokio::test]
async fn a_closing_connection_shuts_its_write_side_once_before_lingering() {
    assert_eq!(shutdowns_after_an_early_answer(false).await, 1);
}

#[tokio::test]
async fn a_closing_upgradeable_connection_shuts_its_write_side_once_before_lingering() {
    assert_eq!(shutdowns_after_an_early_answer(true).await, 1);
}

#[tokio::test]
async fn lingering_after_an_explicit_shutdown_does_not_shut_again() {
    let (server, client) = pair();
    let (server, shutdowns) = Counted::new(server);
    let mut server = ShutOnce::new(server);
    server.shutdown().await.unwrap();
    drop(client);
    linger(server).await;
    assert_eq!(shutdowns.load(std::sync::atomic::Ordering::Relaxed), 1);
}
