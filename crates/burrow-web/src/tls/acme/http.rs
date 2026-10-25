use std::future::Future;
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
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName};
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(30 * 24 * 3600);

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
}

type Failure = Box<dyn std::error::Error + Send + Sync>;

pub fn trusted_roots(ca_pem: Option<&[u8]>) -> Result<RootCertStore, String> {
    let mut roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let Some(pem) = ca_pem else {
        return Ok(roots);
    };
    let certs = CertificateDer::pem_slice_iter(pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("is not a PEM certificate file ({e})"))?;
    if certs.is_empty() {
        return Err("holds no certificate".into());
    }
    for c in certs {
        roots.add(c).map_err(|e| e.to_string())?;
    }
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
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Client {
            tls: TlsConnector::from(Arc::new(config)),
            limits,
            told_to_wait: Arc::new(Mutex::new(None)),
        })
    }

    pub fn take_wait(&self) -> Option<Duration> {
        self.told_to_wait.lock().expect("wait lock").take()
    }

    async fn send(
        &self,
        req: Request<BodyWrapper<Bytes>>,
    ) -> Result<Response<Full<Bytes>>, Failure> {
        let uri = req.uri().clone();
        if uri.scheme_str() != Some("https") {
            return Err(format!("{uri}: only https is allowed").into());
        }
        let host = uri.host().ok_or("no host in the URL")?.to_string();
        let port = uri.port_u16().unwrap_or(443);
        let limit = self.limits.connect;
        let tcp = tokio::time::timeout(
            limit,
            TcpStream::connect((host.trim_matches(['[', ']']), port)),
        )
        .await
        .map_err(|_| format!("connecting to {host}:{port} timed out"))??;
        let name = ServerName::try_from(host.trim_matches(['[', ']']).to_string())?;
        let tls = tokio::time::timeout(limit, self.tls.connect(name, tcp))
            .await
            .map_err(|_| format!("the TLS handshake with {host}:{port} timed out"))??;
        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tls)).await?;
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
        Ok(Response::from_parts(parts, Full::new(body)))
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
            Ok(BytesResponse::from(resp))
        })
    }
}

#[cfg(test)]
mod tests;
