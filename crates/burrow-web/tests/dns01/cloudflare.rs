use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use burrow_web::Secret;
use burrow_web::tls::acme::dns01::Provider;
use burrow_web::tls::acme::dns01::rfc2136::Rfc2136;
use burrow_web::tls::acme::dns01::wire::name;
use bytes::Bytes;
use http::{Method, Response, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::pki_types::PrivateKeyDer;

pub const TOKEN: &str = "cf-test-token";

pub struct Mock {
    pub url: String,
    pub ca: PathBuf,
    pub records: Arc<Mutex<Vec<(String, String, String)>>>,
    _dir: tempfile::TempDir,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn envelope(status: StatusCode, ok: bool, errors: Value, result: Value) -> Response<Full<Bytes>> {
    let body = json!({"success": ok, "errors": errors, "messages": [], "result": result});
    let mut r = Response::new(Full::new(Bytes::from(body.to_string())));
    *r.status_mut() = status;
    r
}

struct State {
    records: Arc<Mutex<Vec<(String, String, String)>>>,
    bind: Rfc2136,
    next: Mutex<u32>,
}

async fn handle(
    s: &State,
    method: Method,
    path: &str,
    auth: Option<String>,
    body: Value,
) -> Response<Full<Bytes>> {
    if auth.as_deref() != Some(&format!("Bearer {TOKEN}")) {
        return envelope(
            StatusCode::FORBIDDEN,
            false,
            json!([{"code": 10000, "message": "Authentication error"}]),
            Value::Null,
        );
    }
    if path.starts_with("/zones?name=") {
        let zone = path.trim_start_matches("/zones?name=");
        let found = if zone == "example.test" {
            json!([{"id": "zone-example", "name": "example.test", "status": "active"}])
        } else {
            json!([])
        };
        return envelope(StatusCode::OK, true, json!([]), found);
    }
    if method == Method::POST {
        let n = body["name"].as_str().unwrap_or_default().to_string();
        let content = body["content"].as_str().unwrap_or_default().to_string();
        let value = content.trim_matches('"').to_string();
        s.bind.present(&name(&n).unwrap(), &value).await.unwrap();
        let mut next = s.next.lock().unwrap();
        *next += 1;
        let id = format!("rec-{next}");
        s.records.lock().unwrap().push((id.clone(), n, content));
        return envelope(StatusCode::OK, true, json!([]), json!({"id": id}));
    }
    if method == Method::DELETE {
        let id = path.rsplit('/').next().unwrap_or_default().to_string();
        let gone = {
            let mut list = s.records.lock().unwrap();
            let at = list.iter().position(|(rid, _, _)| *rid == id);
            at.map(|i| list.remove(i))
        };
        if let Some((_, n, content)) = gone {
            s.bind
                .cleanup(&name(&n).unwrap(), content.trim_matches('"'))
                .await
                .unwrap();
            return envelope(StatusCode::OK, true, json!([]), json!({"id": id}));
        }
        return envelope(
            StatusCode::NOT_FOUND,
            false,
            json!([{"code": 81044, "message": "Record does not exist."}]),
            Value::Null,
        );
    }
    envelope(StatusCode::OK, true, json!([]), json!([]))
}

impl Mock {
    pub async fn start() -> Mock {
        let dir = tempfile::tempdir().unwrap();
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = rcgen::CertificateParams::new(vec!["localhost".to_string()])
            .unwrap()
            .self_signed(&key)
            .unwrap();
        let ca = dir.path().join("cf-ca.pem");
        std::fs::write(&ca, cert.pem()).unwrap();
        let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
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
        let records = Arc::new(Mutex::new(Vec::new()));
        let state = Arc::new(State {
            records: records.clone(),
            bind: Rfc2136::new(
                super::bind::ADDR,
                super::bind::KEY,
                Secret::new(super::bind::SECRET.into()),
            )
            .unwrap(),
            next: Mutex::new(0),
        });
        let task = tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let (acceptor, state) = (acceptor.clone(), state.clone());
                tokio::spawn(async move {
                    let Ok(tls) = acceptor.accept(tcp).await else {
                        return;
                    };
                    let svc = service_fn(move |req: http::Request<hyper::body::Incoming>| {
                        let state = state.clone();
                        async move {
                            let (parts, body) = req.into_parts();
                            let body = body
                                .collect()
                                .await
                                .map(|b| b.to_bytes())
                                .unwrap_or_default();
                            let auth = parts
                                .headers
                                .get("authorization")
                                .and_then(|v| v.to_str().ok())
                                .map(str::to_string);
                            let path = parts
                                .uri
                                .path_and_query()
                                .map_or("/", |p| p.as_str())
                                .to_string();
                            let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                            Ok::<_, std::convert::Infallible>(
                                handle(&state, parts.method, &path, auth, body).await,
                            )
                        }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(tls), svc)
                        .await;
                });
            }
        });
        Mock {
            url: format!("https://localhost:{port}"),
            ca,
            records,
            _dir: dir,
            task,
        }
    }
}
