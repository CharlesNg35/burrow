//! One request to a running burrow's local status endpoint with its token: GET /status for burrow status, POST /clients/update for burrow clients.

use std::future::Future;
use std::net::SocketAddr;
use std::time::Instant;

use crate::instance::publish::Entry;

const MAX_BODY: usize = 16 * 1024 * 1024;

pub enum Fetch {
    Gone,
    Failed,
}

pub type Answer = Result<(u16, Vec<u8>), (Fetch, String)>;

fn failed(e: impl ToString) -> (Fetch, String) {
    (Fetch::Failed, e.to_string())
}

pub fn answered(code: u16) -> String {
    let reason = hyper::StatusCode::from_u16(code)
        .ok()
        .and_then(|c| c.canonical_reason())
        .unwrap_or("");
    format!("it answered HTTP/1.1 {code} {reason}")
        .trim_end()
        .to_string()
}

pub fn blocking(deadline: Instant, ask: impl Future<Output = Answer>) -> Answer {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(failed)?;
    let left = deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| failed("it did not answer in time"))?;
    rt.block_on(async {
        tokio::time::timeout(left, ask)
            .await
            .unwrap_or_else(|_| Err(failed("it did not answer in time")))
    })
}

pub async fn call(entry: &Entry, method: &str, path: &str, body: Vec<u8>) -> Answer {
    use http_body_util::BodyExt;
    let addr = SocketAddr::from(([127, 0, 0, 1], entry.port));
    let s = tokio::net::TcpStream::connect(addr).await.map_err(|e| {
        let kind = if e.kind() == std::io::ErrorKind::ConnectionRefused {
            Fetch::Gone
        } else {
            Fetch::Failed
        };
        (kind, e.to_string())
    })?;
    let (mut sender, conn) = hyper::client::conn::http1::Builder::new()
        .title_case_headers(true)
        .handshake::<_, http_body_util::Full<bytes::Bytes>>(hyper_util::rt::TokioIo::new(s))
        .await
        .map_err(failed)?;
    let driver = tokio::spawn(conn);
    let result = async {
        let mut request = hyper::Request::builder()
            .method(method)
            .uri(path)
            .header("Host", addr.to_string())
            .header("Authorization", format!("Bearer {}", entry.token))
            .header("Connection", "close");
        if !body.is_empty() {
            request = request.header("Content-Type", "application/json");
        }
        let request = request
            .body(http_body_util::Full::new(bytes::Bytes::from(body)))
            .map_err(failed)?;
        let response = sender.send_request(request).await.map_err(|e| {
            if e.is_parse() {
                failed("the answer is not HTTP")
            } else {
                failed(e)
            }
        })?;
        let code = response.status().as_u16();
        let body = http_body_util::Limited::new(response.into_body(), MAX_BODY)
            .collect()
            .await
            .map_err(|e| {
                if e.is::<http_body_util::LengthLimitError>() {
                    failed("the answer is too large")
                } else {
                    failed(e)
                }
            })?
            .to_bytes();
        Ok((code, body.to_vec()))
    }
    .await;
    driver.abort();
    result
}
