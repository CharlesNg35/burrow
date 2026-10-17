use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tracing::{debug, warn};

use super::{Status, prometheus};

const MAX_HEAD: usize = 8 * 1024;
const MAX_OPEN: usize = 32;
const READ_TIMEOUT: Duration = Duration::from_secs(5);

pub type Snapshot = Arc<dyn Fn() -> Status + Send + Sync>;

pub async fn serve(listener: TcpListener, snapshot: Snapshot) {
    let open = Arc::new(Semaphore::new(MAX_OPEN));
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                warn!("metrics: accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let Ok(permit) = open.clone().try_acquire_owned() else {
            debug!(%peer, "metrics: too many open requests, closing");
            continue;
        };
        let snapshot = snapshot.clone();
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(e) = answer(stream, &snapshot).await {
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
        ("GET", "/status") => (
            200,
            "application/json",
            serde_json::to_string_pretty(&snapshot()).expect("status serializes"),
        ),
        ("GET", _) => (
            404,
            "text/plain",
            "not found; try /metrics or /status\n".into(),
        ),
        _ => (405, "text/plain", "only GET is supported\n".into()),
    }
}

async fn answer(mut stream: TcpStream, snapshot: &Snapshot) -> std::io::Result<()> {
    let head = tokio::time::timeout(READ_TIMEOUT, read_head(&mut stream))
        .await
        .map_err(|_| std::io::Error::other("request timed out"))??;
    let (code, kind, body) = respond(&head, snapshot.as_ref());
    let reason = match code {
        200 => "OK",
        404 => "Not Found",
        _ => "Method Not Allowed",
    };
    let response = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> Status {
        Status {
            version: "1.0.0".into(),
            uptime_secs: 5,
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
        let (code, kind, body) = respond(b"GET /status?x=1 HTTP/1.1\r\n\r\n", &empty);
        assert_eq!((code, kind), (200, "application/json"));
        assert_eq!(serde_json::from_str::<Status>(&body).unwrap(), empty());
        assert_eq!(respond(b"GET / HTTP/1.1\r\n\r\n", &empty).0, 404);
        assert_eq!(respond(b"POST /metrics HTTP/1.1\r\n\r\n", &empty).0, 405);
        assert_eq!(respond(b"\r\n\r\n", &empty).0, 405);
    }
}
