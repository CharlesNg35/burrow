use std::sync::Arc;
use std::time::Duration;

use hyper::{Request, Response};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tracing::{debug, warn};

use super::{Status, prometheus};

const MAX_HEAD: usize = 8 * 1024;
const MAX_OPEN: usize = 64;
const MAX_WAITING: usize = 256;
const READ_TIMEOUT: Duration = Duration::from_secs(1);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const WAIT_FOR_SLOT: Duration = Duration::from_secs(2);

pub type Snapshot = Arc<dyn Fn() -> Status + Send + Sync>;
pub type Handler = Arc<dyn Fn(&Request<()>) -> (u16, &'static str, String) + Send + Sync>;

pub async fn serve(listener: TcpListener, snapshot: Snapshot) {
    let handler: Handler = Arc::new(move |req| respond(req, snapshot.as_ref()));
    serve_with(listener, handler).await
}

pub async fn serve_with(listener: TcpListener, handler: Handler) {
    let open = Arc::new(Semaphore::new(MAX_OPEN));
    let waiting = Arc::new(Semaphore::new(MAX_WAITING));
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) if crate::wire::transport::tcp::gone_before_accept(&e) => {
                debug!("metrics: a connection was gone before it was accepted: {e}");
                continue;
            }
            Err(e) => {
                static FAILED: crate::policy::throttle::Throttle =
                    crate::policy::throttle::Throttle::new();
                if let Some(more) = FAILED.allow() {
                    warn!("metrics: accept failed: {e}{more}");
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let Ok(queued) = waiting.clone().try_acquire_owned() else {
            debug!(%peer, "metrics: too many waiting requests, closing");
            continue;
        };
        let (open, handler) = (open.clone(), handler.clone());
        tokio::spawn(async move {
            let slot = tokio::time::timeout(WAIT_FOR_SLOT, open.acquire_owned()).await;
            drop(queued);
            let Ok(Ok(_permit)) = slot else {
                debug!(%peer, "metrics: no free slot, closing");
                return;
            };
            if let Err(e) = answer(stream, &handler).await {
                debug!(%peer, "metrics: {e}");
            }
        });
    }
}

struct Capped {
    inner: TcpStream,
    left: usize,
}

impl tokio::io::AsyncRead for Capped {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let polled = std::pin::Pin::new(&mut self.inner).poll_read(cx, buf);
        let read = buf.filled().len() - before;
        if read > self.left {
            return std::task::Poll::Ready(Err(std::io::Error::other("request header too large")));
        }
        self.left -= read;
        polled
    }
}

impl tokio::io::AsyncWrite for Capped {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

pub fn respond(req: &Request<()>, snapshot: &dyn Fn() -> Status) -> (u16, &'static str, String) {
    match (req.method().as_str(), req.uri().path()) {
        ("GET", "/metrics") => (
            200,
            "text/plain; version=0.0.4; charset=utf-8",
            prometheus(&snapshot()),
        ),
        ("GET", _) => (404, "text/plain", "not found; try /metrics\n".into()),
        _ => (405, "text/plain", "only GET is supported\n".into()),
    }
}

async fn answer(stream: TcpStream, handler: &Handler) -> std::io::Result<()> {
    let handler = handler.clone();
    let service = hyper::service::service_fn(move |req: Request<hyper::body::Incoming>| {
        let (parts, _) = req.into_parts();
        let (code, kind, body) = handler(&Request::from_parts(parts, ()));
        let response = Response::builder()
            .status(code)
            .header("Content-Type", kind)
            .header("Content-Length", body.len())
            .header("Connection", "close")
            .body(http_body_util::Full::new(bytes::Bytes::from(body)));
        async move { response }
    });
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder
        .timer(hyper_util::rt::TokioTimer::new())
        .header_read_timeout(READ_TIMEOUT)
        .max_buf_size(4 * MAX_HEAD)
        .half_close(true)
        .keep_alive(false)
        .title_case_headers(true)
        .auto_date_header(false);
    let io = hyper_util::rt::TokioIo::new(Capped {
        inner: stream,
        left: MAX_HEAD + 1024,
    });
    match tokio::time::timeout(
        READ_TIMEOUT + WRITE_TIMEOUT,
        builder.serve_connection(io, service),
    )
    .await
    {
        Err(_) => Err(std::io::Error::other("the reader is too slow")),
        Ok(r) => r.map_err(std::io::Error::other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> Status {
        Status {
            version: "1.0.0".into(),
            uptime_secs: 5,
            listening: vec![],
            handshakes_refused: 0,
            udp_peers: 0,
            clients: vec![],
            services: vec![],
            visitors: vec![],
            web: None,
            certificates: Vec::new(),
            acme: None,
            local_ca: None,
        }
    }

    fn req(method: &str, uri: &str) -> Request<()> {
        Request::builder().method(method).uri(uri).body(()).unwrap()
    }

    #[test]
    fn routes() {
        let (code, kind, body) = respond(&req("GET", "/metrics"), &empty);
        assert_eq!((code, kind.starts_with("text/plain")), (200, true));
        assert!(body.contains("burrow_uptime_seconds 5"));
        assert_eq!(respond(&req("GET", "/metrics?x=1"), &empty).0, 200);
        assert_eq!(respond(&req("GET", "/status"), &empty).0, 404);
        assert_eq!(respond(&req("GET", "/"), &empty).0, 404);
        assert_eq!(respond(&req("POST", "/metrics"), &empty).0, 405);
        assert_eq!(respond(&req("PATCH", "/metrics"), &empty).0, 405);
    }

    #[tokio::test]
    async fn a_client_that_half_closes_after_its_request_still_gets_the_answer() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(serve(l, Arc::new(empty)));
        for _ in 0..20 {
            let mut s = TcpStream::connect(addr).await.unwrap();
            s.write_all(b"GET /metrics HTTP/1.1\r\nHost: x\r\n\r\n")
                .await
                .unwrap();
            s.shutdown().await.unwrap();
            let mut out = Vec::new();
            tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out))
                .await
                .unwrap()
                .unwrap();
            let text = String::from_utf8_lossy(&out);
            assert!(text.starts_with("HTTP/1.1 200"), "{text}");
            assert!(text.contains("burrow_uptime_seconds 5"), "{text}");
        }
    }

    #[tokio::test]
    async fn a_garbage_request_gets_no_metrics() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(serve(l, Arc::new(empty)));
        let mut s = TcpStream::connect(addr).await.unwrap();
        s.write_all(b"\r\n\r\nnot http\r\n\r\n").await.unwrap();
        let mut out = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await;
        let text = String::from_utf8_lossy(&out);
        assert!(!text.contains("burrow_uptime_seconds"), "{text}");
        assert!(
            text.is_empty() || text.starts_with("HTTP/1.1 400"),
            "{text}"
        );
    }
}
