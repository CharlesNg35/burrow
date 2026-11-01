use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use burrow::wire::secret::Secret;
use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Full};
use tokio::net::TcpListener;
use tokio_rustls::rustls::pki_types::ServerName;

use burrow::config::settings::TargetTls;

use crate::common::*;
use crate::kit::*;
use crate::sites::with_files;

type Accepted = Arc<AtomicUsize>;

fn accepted(n: &Accepted) -> usize {
    n.load(Ordering::SeqCst)
}

async fn named_app(name: &'static str) -> (SocketAddr, Accepted) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let count = Accepted::default();
    let counted = count.clone();
    tokio::spawn(async move {
        while let Ok((s, _)) = l.accept().await {
            counted.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let service = hyper::service::service_fn(move |_req| async move {
                    Ok::<_, Infallible>(hyper::Response::new(Full::new(Bytes::from(name))))
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(s), service)
                    .await;
            });
        }
    });
    (addr, count)
}

enum Visitor {
    H1(hyper::client::conn::http1::SendRequest<Empty<Bytes>>),
    H2(h2::client::SendRequest<Bytes>),
}

impl Visitor {
    async fn open(web: SocketAddr, cert: &Cert, h2: bool) -> Visitor {
        let tcp = tokio::net::TcpStream::connect(web).await.unwrap();
        let alpn: &[&[u8]] = if h2 { &[b"h2"] } else { &[b"http/1.1"] };
        let name = ServerName::try_from("localhost").unwrap();
        let tls = trusting_alpn(cert, alpn).connect(name, tcp).await.unwrap();
        if h2 {
            let (c, conn) = h2::client::handshake(tls).await.unwrap();
            tokio::spawn(async move {
                let _ = conn.await;
            });
            return Visitor::H2(c);
        }
        let io = hyper_util::rt::TokioIo::new(tls);
        let (c, conn) = hyper::client::conn::http1::handshake(io).await.unwrap();
        tokio::spawn(async move {
            let _ = conn.await;
        });
        Visitor::H1(c)
    }

    async fn get(&mut self) -> String {
        match self {
            Visitor::H1(c) => {
                c.ready().await.unwrap();
                let req = http::Request::get("/")
                    .header("host", "localhost")
                    .body(Empty::new())
                    .unwrap();
                let resp = c.send_request(req).await.unwrap();
                let body = resp.into_body().collect().await.unwrap().to_bytes();
                String::from_utf8_lossy(&body).into_owned()
            }
            Visitor::H2(c) => {
                let mut c = c.clone().ready().await.unwrap();
                let req = http::Request::get("https://localhost/").body(()).unwrap();
                let (resp, _) = c.send_request(req, true).unwrap();
                let mut body = resp.await.unwrap().into_body();
                let mut out = Vec::new();
                while let Some(Ok(chunk)) = body.data().await {
                    let _ = body.flow_control().release_capacity(chunk.len());
                    out.extend_from_slice(&chunk);
                }
                String::from_utf8_lossy(&out).into_owned()
            }
        }
    }
}

async fn a_kept_visitor_reaches_the_new_target_after_a_reload(h2: bool) {
    let ((old, old_conns), (new, new_conns)) = (named_app("old").await, named_app("new").await);
    let dir = tempfile::tempdir().unwrap();
    let site_cert = cert(&["localhost"]);
    let secret = Secret::generate();
    let mut s = web_server(&secret, &[("blog", site(&["home"], old), &["localhost"])]);
    with_files(&mut s, "blog", &site_cert, dir.path());
    s.web.as_mut().unwrap().https = Some(format!("127.0.0.1:{}", free_port()).parse().unwrap());
    let t = tunnel_with(s.clone(), Vec::new(), |c| c, secret).await;
    let mut v = Visitor::open(t.server.web_addr.unwrap(), &site_cert, h2).await;
    assert_eq!(v.get().await, "old");
    assert_eq!(v.get().await, "old");
    assert_eq!(accepted(&old_conns), 1, "the app connection is reused");
    let mut next = s.clone();
    next.services.get_mut("blog").unwrap().target = new.to_string();
    t.server.reload_applied(next).await;
    assert_eq!(
        v.get().await,
        "new",
        "the next request on the same visitor connection reaches the new target"
    );
    assert_eq!(accepted(&new_conns), 1, "over a new app connection");
    assert_eq!(accepted(&old_conns), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_kept_http1_visitor_reaches_the_new_target_after_a_reload() {
    a_kept_visitor_reaches_the_new_target_after_a_reload(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_kept_http2_visitor_reaches_the_new_target_after_a_reload() {
    a_kept_visitor_reaches_the_new_target_after_a_reload(true).await;
}

async fn plain_or_tls_app(c: &Cert) -> (SocketAddr, Accepted) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let tls = acceptor(c);
    let count = Accepted::default();
    let counted = count.clone();
    tokio::spawn(async move {
        while let Ok((s, _)) = l.accept().await {
            counted.fetch_add(1, Ordering::SeqCst);
            let tls = tls.clone();
            tokio::spawn(async move {
                let mut first = [0u8; 1];
                let _ = s.peek(&mut first).await;
                let answer = |name: &'static str| {
                    hyper::service::service_fn(move |_req| async move {
                        Ok::<_, Infallible>(hyper::Response::new(Full::new(Bytes::from(name))))
                    })
                };
                let http = hyper::server::conn::http1::Builder::new();
                if first[0] == 0x16 {
                    let Ok(t) = tls.accept(s).await else { return };
                    let io = hyper_util::rt::TokioIo::new(t);
                    let _ = http.serve_connection(io, answer("tls")).await;
                } else {
                    let io = hyper_util::rt::TokioIo::new(s);
                    let _ = http.serve_connection(io, answer("plain")).await;
                }
            });
        }
    });
    (addr, count)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_kept_visitor_uses_a_new_app_connection_once_a_reload_turns_target_tls_on() {
    let app_cert = cert(&["app.internal"]);
    let (app, conns) = plain_or_tls_app(&app_cert).await;
    let dir = tempfile::tempdir().unwrap();
    let site_cert = cert(&["localhost"]);
    let secret = Secret::generate();
    let mut s = web_server(&secret, &[("blog", site(&["home"], app), &["localhost"])]);
    with_files(&mut s, "blog", &site_cert, dir.path());
    s.web.as_mut().unwrap().https = Some(format!("127.0.0.1:{}", free_port()).parse().unwrap());
    let t = tunnel_with(s.clone(), Vec::new(), |c| c, secret).await;
    let mut v = Visitor::open(t.server.web_addr.unwrap(), &site_cert, false).await;
    assert_eq!(v.get().await, "plain");
    assert_eq!(v.get().await, "plain");
    assert_eq!(accepted(&conns), 1, "the app connection is reused");
    let mut next = s.clone();
    let site = next.web.as_mut().unwrap().sites.get_mut("blog").unwrap();
    site.options.target_tls = TargetTls::Unchecked;
    site.options.target_name = "app.internal".into();
    t.server.reload_applied(next).await;
    assert_eq!(
        v.get().await,
        "tls",
        "the next request on the same visitor connection speaks TLS to the app"
    );
    assert_eq!(accepted(&conns), 2, "over a new app connection");
}
