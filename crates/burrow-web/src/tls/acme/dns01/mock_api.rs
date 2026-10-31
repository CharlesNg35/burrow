#![cfg(test)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use http::{HeaderMap, Method, Response, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::pki_types::PrivateKeyDer;

use crate::tls::acme::http::{Client, Limits};

#[derive(Debug, Clone)]
pub struct Seen {
    pub method: Method,
    pub path: String,
    pub headers: HeaderMap,
    pub body: Value,
}

pub type Handler = dyn Fn(&Seen) -> (StatusCode, Value) + Send + Sync;

pub struct Api {
    pub url: String,
    pub seen: Arc<Mutex<Vec<Seen>>>,
    ca: PathBuf,
    _dir: tempfile::TempDir,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Api {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Api {
    pub async fn start(handler: Arc<Handler>) -> Api {
        Api::listen(handler, false).await
    }

    pub async fn start_plain(handler: Arc<Handler>) -> Api {
        Api::listen(handler, true).await
    }

    async fn listen(handler: Arc<Handler>, plain: bool) -> Api {
        let dir = tempfile::tempdir().unwrap();
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = rcgen::CertificateParams::new(vec!["localhost".to_string()])
            .unwrap()
            .self_signed(&key)
            .unwrap();
        let ca = dir.path().join("ca.pem");
        std::fs::write(&ca, cert.pem()).unwrap();
        let provider = crate::tls::provider();
        let config = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert.der().clone()],
                PrivateKeyDer::try_from(key.serialize_der()).unwrap(),
            )
            .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let task = tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let (acceptor, handler, log) = (acceptor.clone(), handler.clone(), log.clone());
                tokio::spawn(async move {
                    match plain {
                        true => serve(tcp, handler, log).await,
                        false => {
                            if let Ok(tls) = acceptor.accept(tcp).await {
                                serve(tls, handler, log).await;
                            }
                        }
                    }
                });
            }
        });
        let scheme = if plain {
            "http://127.0.0.1"
        } else {
            "https://localhost"
        };
        Api {
            url: format!("{scheme}:{port}"),
            seen,
            ca,
            _dir: dir,
            task,
        }
    }

    pub fn client(&self) -> Client {
        Client::new(Some(&self.ca), Limits::default()).unwrap()
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

async fn serve<I>(io: I, handler: Arc<Handler>, log: Arc<Mutex<Vec<Seen>>>)
where
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let svc = service_fn(move |req: http::Request<hyper::body::Incoming>| {
        let (handler, log) = (handler.clone(), log.clone());
        async move {
            let (parts, body) = req.into_parts();
            let body = body
                .collect()
                .await
                .map(|b| b.to_bytes())
                .unwrap_or_default();
            let seen = Seen {
                method: parts.method,
                path: parts
                    .uri
                    .path_and_query()
                    .map_or("/", |p| p.as_str())
                    .to_string(),
                headers: parts.headers,
                body: serde_json::from_slice(&body).unwrap_or(Value::Null),
            };
            let (status, out) = handler(&seen);
            log.lock().unwrap().push(seen);
            let mut resp = Response::new(Full::new(Bytes::from(out.to_string())));
            *resp.status_mut() = status;
            if status == StatusCode::TOO_MANY_REQUESTS {
                resp.headers_mut()
                    .insert("retry-after", "300".parse().unwrap());
            }
            Ok::<_, std::convert::Infallible>(resp)
        }
    });
    let _ = hyper::server::conn::http1::Builder::new()
        .serve_connection(TokioIo::new(io), svc)
        .await;
}

pub fn envelope(result: Value) -> (StatusCode, Value) {
    (
        StatusCode::OK,
        json!({"success": true, "errors": [], "messages": [], "result": result}),
    )
}

pub fn failure(status: StatusCode, code: i64, message: &str) -> (StatusCode, Value) {
    (
        status,
        json!({"success": false, "errors": [{"code": code, "message": message}], "messages": [], "result": null}),
    )
}
