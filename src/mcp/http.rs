use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, LengthLimitError, Limited, combinators::BoxBody};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tokio::sync::{Semaphore, watch};

use super::tools::Tools;
use super::{HttpOptions, setup, token_file};
use crate::base::failure::Failure;

pub const ENDPOINT: &str = "/mcp";
pub const MAX_BODY: usize = 1024 * 1024;
pub const MAX_OPEN: usize = 8;
pub const HEADER_TIMEOUT: Duration = Duration::from_secs(5);
pub const BODY_TIMEOUT: Duration = Duration::from_secs(10);
const SLACK: Duration = Duration::from_secs(10);
const MAX_HEAD: usize = 16 * 1024;

type Body = BoxBody<Bytes, Infallible>;
type Mcp = StreamableHttpService<Tools, NeverSessionManager>;

fn plain(code: StatusCode, text: &str, extra: Option<(&str, &str)>) -> Response<Body> {
    let mut b = Response::builder()
        .status(code)
        .header("Content-Type", "text/plain; charset=utf-8")
        .header("Cache-Control", "no-store")
        .header("Connection", "close");
    if let Some((k, v)) = extra {
        b = b.header(k, v);
    }
    b.body(Full::new(Bytes::from(format!("{text}\n"))).boxed())
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::new()).boxed()))
}

pub fn bearer<B>(req: &Request<B>) -> Option<&str> {
    let v = req
        .headers()
        .get(hyper::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let (scheme, token) = v.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
}

async fn handle(req: Request<Incoming>, token: &Path, mcp: &Mcp) -> Response<Body> {
    if req.uri().path() != ENDPOINT {
        return plain(StatusCode::NOT_FOUND, "not found", None);
    }
    if !bearer(&req).is_some_and(|t| token_file::matches(token, t)) {
        return plain(
            StatusCode::UNAUTHORIZED,
            "a bearer token is required",
            Some(("WWW-Authenticate", "Bearer")),
        );
    }
    let (parts, body) = req.into_parts();
    let read = Limited::new(body, MAX_BODY).collect();
    let bytes = match tokio::time::timeout(BODY_TIMEOUT, read).await {
        Ok(Ok(b)) => b.to_bytes(),
        Ok(Err(e)) if e.is::<LengthLimitError>() => {
            return plain(
                StatusCode::PAYLOAD_TOO_LARGE,
                "the request body is too large",
                None,
            );
        }
        Ok(Err(_)) => return plain(StatusCode::BAD_REQUEST, "the request body broke off", None),
        Err(_) => {
            return plain(
                StatusCode::REQUEST_TIMEOUT,
                "the request body came too slowly",
                None,
            );
        }
    };
    mcp.handle(Request::from_parts(parts, Full::new(bytes)))
        .await
}

fn service(tools: Tools, port: u16) -> Mcp {
    let local = |scheme: &str| {
        ["127.0.0.1", "localhost"]
            .map(|h| format!("{scheme}{h}:{port}"))
            .to_vec()
    };
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_allowed_hosts(local(""))
        .with_allowed_origins(local("http://"))
        .enforce_origin_validation()
        .with_max_request_body_bytes(MAX_BODY);
    StreamableHttpService::new(
        move || Ok(tools.clone()),
        Arc::new(NeverSessionManager::default()),
        config,
    )
}

async fn connection(stream: tokio::net::TcpStream, token: Arc<PathBuf>, mcp: Mcp) {
    let svc = service_fn(move |req| {
        let (token, mcp) = (token.clone(), mcp.clone());
        async move { Ok::<_, Infallible>(handle(req, &token, &mcp).await) }
    });
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_TIMEOUT)
        .max_buf_size(MAX_HEAD)
        .keep_alive(false)
        .auto_date_header(false);
    let longest = HEADER_TIMEOUT + BODY_TIMEOUT + crate::admin_link::client::ANSWER + SLACK;
    let served = builder.serve_connection(TokioIo::new(stream), svc);
    let _ = tokio::time::timeout(longest, served).await;
}

pub struct Started<'a> {
    pub token: &'a Path,
    pub show_token: bool,
    pub config: Option<&'a Path>,
}

pub async fn serve(
    tools: Tools,
    opts: &HttpOptions,
    shown: &Started<'_>,
    mut stop: watch::Receiver<bool>,
) -> Result<(), Failure> {
    let token = token_file::load_or_create(shown.token)?;
    let listener = crate::admin_link::bind(opts.port).await?;
    let port = listener
        .local_addr()
        .map_err(|e| Failure::new(format!("cannot read the local address: {e}")))?
        .port();
    let mcp = service(tools, port);
    let shown_token = shown.show_token.then_some(&token);
    crate::say_err!("{}", setup::started(port, shown_token, shown.config));
    drop(token);
    let path = Arc::new(shown.token.to_path_buf());
    let open = Arc::new(Semaphore::new(MAX_OPEN));
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                };
                let Ok(permit) = open.clone().try_acquire_owned() else {
                    continue;
                };
                let (path, mcp) = (path.clone(), mcp.clone());
                tokio::spawn(async move {
                    let _permit = permit;
                    connection(stream, path, mcp).await;
                });
            }
            _ = stop.wait_for(|s| *s) => break,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bearer_scheme_is_case_insensitive_and_nothing_else_counts() {
        let req = |v: Option<&str>| {
            let mut b = Request::builder().uri(ENDPOINT);
            if let Some(v) = v {
                b = b.header("Authorization", v);
            }
            b.body(()).unwrap()
        };
        assert_eq!(bearer(&req(Some("Bearer abc"))), Some("abc"));
        assert_eq!(bearer(&req(Some("bearer abc"))), Some("abc"));
        assert_eq!(bearer(&req(Some("Basic abc"))), None);
        assert_eq!(bearer(&req(Some("abc"))), None);
        assert_eq!(bearer(&req(None)), None);
    }
}
