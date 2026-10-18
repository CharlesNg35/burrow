use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
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
pub type Handler = Arc<dyn Fn(&[u8]) -> (u16, &'static str, String) + Send + Sync>;

pub async fn serve(listener: TcpListener, snapshot: Snapshot) {
    let handler: Handler = Arc::new(move |head| respond(head, snapshot.as_ref()));
    serve_with(listener, handler).await
}

pub async fn serve_with(listener: TcpListener, handler: Handler) {
    let open = Arc::new(Semaphore::new(MAX_OPEN));
    let waiting = Arc::new(Semaphore::new(MAX_WAITING));
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) if crate::transport::tcp::gone_before_accept(&e) => {
                debug!("metrics: a connection was gone before it was accepted: {e}");
                continue;
            }
            Err(e) => {
                static FAILED: crate::throttle::Throttle = crate::throttle::Throttle::new();
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

async fn read_head(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut head = Vec::with_capacity(512);
    let mut chunk = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        if head.len() > MAX_HEAD {
            return Err(std::io::Error::other("request header too large"));
        }
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Err(std::io::Error::other("closed before the request ended"));
        }
        head.extend_from_slice(&chunk[..n]);
    }
    Ok(head)
}

pub fn respond(head: &[u8], snapshot: &dyn Fn() -> Status) -> (u16, &'static str, String) {
    let line = head.split(|b| *b == b'\r').next().unwrap_or_default();
    let line = String::from_utf8_lossy(line);
    let mut parts = line.split(' ');
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let path = path.split('?').next().unwrap_or("");
    match (method, path) {
        ("GET", "/metrics") => (
            200,
            "text/plain; version=0.0.4; charset=utf-8",
            prometheus(&snapshot()),
        ),
        ("GET", _) => (404, "text/plain", "not found; try /metrics\n".into()),
        _ => (405, "text/plain", "only GET is supported\n".into()),
    }
}

async fn answer(mut stream: TcpStream, handler: &Handler) -> std::io::Result<()> {
    let head = tokio::time::timeout(READ_TIMEOUT, read_head(&mut stream))
        .await
        .map_err(|_| std::io::Error::other("request timed out"))??;
    let (code, kind, body) = handler(&head);
    let write = async {
        let reason = match code {
            200 => "OK",
            401 => "Unauthorized",
            404 => "Not Found",
            _ => "Method Not Allowed",
        };
        let response = format!(
            "HTTP/1.1 {code} {reason}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await?;
        stream.shutdown().await
    };
    tokio::time::timeout(WRITE_TIMEOUT, write)
        .await
        .map_err(|_| std::io::Error::other("the reader is too slow"))?
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
        }
    }

    #[test]
    fn routes() {
        let (code, kind, body) = respond(b"GET /metrics HTTP/1.1\r\n\r\n", &empty);
        assert_eq!((code, kind.starts_with("text/plain")), (200, true));
        assert!(body.contains("burrow_uptime_seconds 5"));
        assert_eq!(respond(b"GET /status HTTP/1.1\r\n\r\n", &empty).0, 404);
        assert_eq!(respond(b"GET / HTTP/1.1\r\n\r\n", &empty).0, 404);
        assert_eq!(respond(b"POST /metrics HTTP/1.1\r\n\r\n", &empty).0, 405);
        assert_eq!(respond(b"\r\n\r\n", &empty).0, 405);
    }
}
