use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use burrow_web::tls::{Certificates, Fixed, Store};
use bytes::Bytes;
use http_body_util::Full;
use hyper_util::rt::TokioIo;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::LazyConfigAcceptor;
use tokio_rustls::rustls::server::{Acceptor, ResolvesServerCert};
use tokio_rustls::rustls::{self, ServerConfig};

pub struct Responder {
    pub current: Arc<RwLock<Arc<Store>>>,
    pub refuse_tls_check: Arc<AtomicBool>,
    pub https_in_front: Arc<AtomicBool>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for Responder {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

fn config(resolver: Arc<dyn ResolvesServerCert>, alpn: &[u8]) -> Arc<ServerConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut c = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_cert_resolver(resolver);
    c.alpn_protocols = vec![alpn.to_vec()];
    Arc::new(c)
}

fn front_config() -> Arc<ServerConfig> {
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["cdn.example.net".to_string()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let c = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.der().clone()],
            tokio_rustls::rustls::pki_types::PrivateKeyDer::try_from(key.serialize_der()).unwrap(),
        )
        .unwrap();
    Arc::new(c)
}

async fn tls_conn(store: Arc<Store>, refuse: bool, front: bool, tcp: TcpStream) {
    let Ok(start) = LazyConfigAcceptor::new(Acceptor::default(), tcp).await else {
        return;
    };
    if front {
        if let Ok(mut tls) = start.into_stream(front_config()).await {
            let _ = tls.shutdown().await;
        }
        return;
    }
    let hello = start.client_hello();
    let alpn: Vec<Vec<u8>> = hello
        .alpn()
        .map(|a| a.map(<[u8]>::to_vec).collect())
        .unwrap_or_default();
    let sni = hello.server_name().map(str::to_string);
    if alpn == [b"acme-tls/1".to_vec()] {
        if refuse {
            return;
        }
        let Some(key) = store.alpn_challenge(sni.as_deref().unwrap_or_default()) else {
            return;
        };
        if let Ok(mut tls) = start
            .into_stream(config(Arc::new(Fixed::new(key)), b"acme-tls/1"))
            .await
        {
            let _ = tls.shutdown().await;
        }
        return;
    }
    let resolver: Arc<dyn ResolvesServerCert> = match sni {
        Some(_) => store,
        None => match store.certificate("127.0.0.1") {
            Some(k) => Arc::new(Fixed::new(k)),
            None => return,
        },
    };
    if let Ok(mut tls) = start.into_stream(config(resolver, b"http/1.1")).await {
        let _ = tls.write_all(b"ok").await;
        let _ = tls.shutdown().await;
    }
}

async fn http_conn(store: Arc<Store>, tcp: TcpStream) {
    let svc = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
        let token = req
            .uri()
            .path()
            .strip_prefix("/.well-known/acme-challenge/")
            .map(str::to_string);
        let answer = token.and_then(|t| store.http_challenge(&t));
        async move {
            Ok::<_, Infallible>(match answer {
                Some(a) => hyper::Response::new(Full::new(Bytes::from(a))),
                None => hyper::Response::builder()
                    .status(404)
                    .body(Full::new(Bytes::new()))
                    .unwrap(),
            })
        }
    });
    let _ = hyper::server::conn::http1::Builder::new()
        .serve_connection(TokioIo::new(tcp), svc)
        .await;
}

pub async fn start(store: Arc<Store>, tls_port: u16, http_port: u16) -> Responder {
    let current = Arc::new(RwLock::new(store));
    let refuse = Arc::new(AtomicBool::new(false));
    let front = Arc::new(AtomicBool::new(false));
    let tls = TcpListener::bind(("127.0.0.1", tls_port)).await.unwrap();
    let http = TcpListener::bind(("127.0.0.1", http_port)).await.unwrap();
    let (c1, r1, f1) = (current.clone(), refuse.clone(), front.clone());
    let t1 = tokio::spawn(async move {
        while let Ok((tcp, _)) = tls.accept().await {
            let store = c1.read().unwrap().clone();
            let (r, f) = (r1.load(Ordering::SeqCst), f1.load(Ordering::SeqCst));
            tokio::spawn(tls_conn(store, r, f, tcp));
        }
    });
    let c2 = current.clone();
    let t2 = tokio::spawn(async move {
        while let Ok((tcp, _)) = http.accept().await {
            let store = c2.read().unwrap().clone();
            tokio::spawn(http_conn(store, tcp));
        }
    });
    Responder {
        current,
        refuse_tls_check: refuse,
        https_in_front: front,
        tasks: vec![t1, t2],
    }
}

impl Responder {
    pub fn serve(&self, store: Arc<Store>) {
        *self.current.write().unwrap() = store;
    }

    pub fn https_in_front(&self, on: bool) {
        self.https_in_front.store(on, Ordering::SeqCst);
    }

    pub fn refuse_tls(&self, on: bool) {
        self.refuse_tls_check.store(on, Ordering::SeqCst);
    }
}
