use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use http::header::{HOST, RETRY_AFTER};
use http::{HeaderMap, HeaderValue, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full, Limited};
use hyper_util::rt::TokioIo;
use instant_acme::{BodyWrapper, BytesResponse, HttpClient};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(30 * burrow_syntax::duration::DAY_SECS);

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub connect: Duration,
    pub request: Duration,
    pub body: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            connect: Duration::from_secs(10),
            request: Duration::from_secs(60),
            body: 1024 * 1024,
        }
    }
}

#[derive(Clone)]
pub struct Client {
    tls: TlsConnector,
    limits: Limits,
    told_to_wait: Arc<Mutex<Option<Duration>>>,
    plain_private: bool,
}

pub fn is_private(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private(),
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unique_local(),
    }
}

pub async fn private_addr(host: &str, port: u16) -> Result<SocketAddr, String> {
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| format!("cannot find {host}: {e}"))?
        .collect();
    if let Some(public) = addrs.iter().find(|a| !is_private(a.ip())) {
        let ip = public.ip().to_string();
        let what = match host == ip {
            true => format!("{host} is a public address"),
            false => format!("{host} is {ip}, a public address"),
        };
        return Err(format!(
            "{what}; plain http:// is allowed only to loopback or private addresses (10/8, 172.16/12, 192.168/16, fc00::/7), so use https://"
        ));
    }
    addrs
        .first()
        .copied()
        .ok_or_else(|| format!("{host} has no address"))
}

type Failure = Box<dyn std::error::Error + Send + Sync>;

pub fn trusted_roots(ca_pem: Option<&[u8]>) -> Result<RootCertStore, String> {
    let mut roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let Some(pem) = ca_pem else {
        return Ok(roots);
    };
    roots.roots.extend(crate::tls::files::ca_roots(pem)?.roots);
    Ok(roots)
}

impl Client {
    pub fn new(ca: Option<&Path>, limits: Limits) -> Result<Client, String> {
        let pem = ca
            .map(|ca| std::fs::read(ca).map_err(|e| format!("cannot read {}: {e}", ca.display())))
            .transpose()?;
        let roots = trusted_roots(pem.as_deref()).map_err(|e| {
            format!(
                "{}: {e}",
                ca.map_or(String::new(), |c| c.display().to_string())
            )
        })?;
        let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
        let mut config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_root_certificates(roots)
            .with_no_client_auth();
        config.alpn_protocols = vec![crate::terminate::HTTP_1_1.to_vec()];
        Ok(Client {
            tls: TlsConnector::from(Arc::new(config)),
            limits,
            told_to_wait: Arc::new(Mutex::new(None)),
            plain_private: false,
        })
    }

    pub fn with_plain_private(mut self) -> Client {
        self.plain_private = true;
        self
    }

    pub async fn fetch(&self, req: Request<Vec<u8>>) -> Result<Response<Bytes>, String> {
        let (parts, body) = req.into_parts();
        let req = Request::from_parts(parts, BodyWrapper::from(body));
        tokio::time::timeout(self.limits.request, self.send(req))
            .await
            .map_err(|_| "no answer in time".to_string())?
            .map_err(|e| e.to_string())
    }

    pub fn take_wait(&self) -> Option<Duration> {
        self.told_to_wait.lock().expect("wait lock").take()
    }

    async fn send(&self, req: Request<BodyWrapper<Bytes>>) -> Result<Response<Bytes>, Failure> {
        let uri = req.uri().clone();
        let host = uri
            .host()
            .ok_or("no host in the URL")?
            .trim_matches(['[', ']'])
            .to_string();
        let limit = self.limits.connect;
        let late = |port: u16| format!("connecting to {host}:{port} timed out");
        match uri.scheme_str() {
            Some("https") => {
                let port = uri.port_u16().unwrap_or(443);
                let tcp = tokio::time::timeout(limit, TcpStream::connect((host.as_str(), port)))
                    .await
                    .map_err(|_| late(port))??;
                let name = ServerName::try_from(host.clone())?;
                let tls = tokio::time::timeout(limit, self.tls.connect(name, tcp))
                    .await
                    .map_err(|_| format!("the TLS handshake with {host}:{port} timed out"))??;
                self.exchange(tls, &uri, req).await
            }
            Some("http") if self.plain_private => {
                let port = uri.port_u16().unwrap_or(80);
                let addr = private_addr(&host, port).await?;
                let tcp = tokio::time::timeout(limit, TcpStream::connect(addr))
                    .await
                    .map_err(|_| late(port))??;
                self.exchange(tcp, &uri, req).await
            }
            _ => Err(format!("{uri}: only https is allowed").into()),
        }
    }

    async fn exchange<I>(
        &self,
        io: I,
        uri: &http::Uri,
        req: Request<BodyWrapper<Bytes>>,
    ) -> Result<Response<Bytes>, Failure>
    where
        I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(io)).await?;
        let _driver = Driver(tokio::spawn(conn));
        let (mut parts, body) = req.into_parts();
        let authority = uri.authority().ok_or("no host in the URL")?.as_str();
        parts
            .headers
            .insert(HOST, HeaderValue::from_str(authority)?);
        parts.uri = uri.path_and_query().map_or("/", |p| p.as_str()).parse()?;
        let resp = sender
            .send_request(Request::from_parts(parts, body))
            .await?;
        drop(sender);
        let (parts, body) = resp.into_parts();
        let body = Limited::new(body, self.limits.body)
            .collect()
            .await
            .map_err(|e| format!("reading the answer: {e}"))?
            .to_bytes();
        Ok(Response::from_parts(parts, body))
    }
}

struct Driver<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for Driver<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub fn clamp_retry_after(headers: &mut HeaderMap, now: SystemTime) -> Option<Duration> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok().map(str::trim);
    let wait = value.and_then(|v| match u64::from_str(v) {
        Ok(secs) => Some(Duration::from_secs(secs)),
        Err(_) => httpdate::parse_http_date(v)
            .ok()
            .map(|at| at.duration_since(now).unwrap_or(Duration::ZERO)),
    });
    match wait {
        Some(w) => {
            let w = w.min(MAX_RETRY_AFTER);
            headers.insert(RETRY_AFTER, HeaderValue::from(w.as_secs()));
            Some(w)
        }
        None => {
            headers.remove(RETRY_AFTER);
            None
        }
    }
}

impl HttpClient for Client {
    fn request(
        &self,
        req: Request<BodyWrapper<Bytes>>,
    ) -> Pin<Box<dyn Future<Output = Result<BytesResponse, instant_acme::Error>> + Send>> {
        let me = self.clone();
        Box::pin(async move {
            let sent = tokio::time::timeout(me.limits.request, me.send(req)).await;
            let mut resp = match sent {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => return Err(instant_acme::Error::Other(e)),
                Err(_) => {
                    return Err(instant_acme::Error::Other(
                        "the ACME server did not answer in time".into(),
                    ));
                }
            };
            let wait = clamp_retry_after(resp.headers_mut(), SystemTime::now());
            let status = resp.status();
            if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::SERVICE_UNAVAILABLE
            {
                *me.told_to_wait.lock().expect("wait lock") = wait;
            }
            #[cfg(any(test, feature = "admin-test-hooks"))]
            count_nonce_refusal(&resp);
            Ok(BytesResponse::from(resp.map(Full::new)))
        })
    }
}

#[cfg(any(test, feature = "admin-test-hooks"))]
static NONCE_REFUSALS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(any(test, feature = "admin-test-hooks"))]
fn count_nonce_refusal(resp: &Response<Bytes>) {
    let refused = resp.status() == StatusCode::BAD_REQUEST
        && serde_json::from_slice::<instant_acme::Problem>(resp.body())
            .is_ok_and(|p| p.r#type.as_deref() == Some("urn:ietf:params:acme:error:badNonce"));
    if refused {
        NONCE_REFUSALS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(any(test, feature = "admin-test-hooks"))]
pub fn nonce_refusals() -> u64 {
    NONCE_REFUSALS.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
mod tests;
