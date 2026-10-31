use std::convert::Infallible;
use std::net::SocketAddr;
use std::time::Duration;

use burrow::wire::secret::Secret;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use tokio::net::TcpListener;
use tokio_rustls::rustls::pki_types::ServerName;

use crate::common::*;
use crate::kit::*;
use crate::sites::with_files;

const UPLOADS: usize = 16;
const MIB: usize = 1024 * 1024;

async fn upload_app() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((s, _)) = l.accept().await {
            tokio::spawn(async move {
                let service = hyper::service::service_fn(
                    |req: hyper::Request<hyper::body::Incoming>| async move {
                        let n = req
                            .into_body()
                            .collect()
                            .await
                            .map_or(0, |b| b.to_bytes().len());
                        Ok::<_, Infallible>(hyper::Response::new(Full::new(Bytes::from(format!(
                            "got {n}"
                        )))))
                    },
                );
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(s), service)
                    .await;
            });
        }
    });
    addr
}

async fn upload(c: h2::client::SendRequest<Bytes>) -> (u16, String) {
    let mut c = c.ready().await.unwrap();
    let req = http::Request::post("https://localhost/up")
        .body(())
        .unwrap();
    let (resp, mut tx) = c.send_request(req, false).unwrap();
    tx.reserve_capacity(MIB);
    tx.send_data(Bytes::from(vec![b'u'; MIB]), true).unwrap();
    let resp = resp.await.unwrap();
    let status = resp.status().as_u16();
    let mut body = resp.into_body();
    let mut text = Vec::new();
    while let Some(Ok(chunk)) = body.data().await {
        let _ = body.flow_control().release_capacity(chunk.len());
        text.extend_from_slice(&chunk);
    }
    (status, String::from_utf8_lossy(&text).into_owned())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn uploads_on_one_h2_connection_past_the_services_max_connections_get_503_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let site_cert = cert(&["localhost"]);
    let secret = Secret::generate();
    let mut limited = site(&["home"], upload_app().await);
    limited.max_connections = Some(4);
    let mut s = web_server(&secret, &[("blog", limited, &["localhost"])]);
    with_files(&mut s, "blog", &site_cert, dir.path());
    s.web.as_mut().unwrap().https = Some(format!("127.0.0.1:{}", free_port()).parse().unwrap());
    let t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    let web = t.server.web_addr.unwrap();
    let tcp = tokio::net::TcpStream::connect(web).await.unwrap();
    let name = ServerName::try_from("localhost").unwrap();
    let tls = trusting_alpn(&site_cert, &[b"h2"])
        .connect(name, tcp)
        .await
        .unwrap();
    assert_eq!(tls.get_ref().1.alpn_protocol(), Some(&b"h2"[..]));
    let (c, conn) = h2::client::handshake(tls).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let tasks: Vec<_> = (0..UPLOADS)
        .map(|_| tokio::spawn(upload(c.clone())))
        .collect();
    let all = async {
        let mut got = Vec::new();
        for task in tasks {
            got.push(task.await.unwrap());
        }
        got
    };
    let got = tokio::time::timeout(Duration::from_secs(20), all)
        .await
        .expect("no upload waits for the 60 s answer timeout");
    let done = got
        .iter()
        .filter(|(s, b)| *s == 200 && b == "got 1048576")
        .count();
    let busy = got.iter().filter(|(s, _)| *s == 503).count();
    assert!(done >= 1, "{got:?}");
    assert!(busy >= 1, "max_connections: 4 refuses some of 16: {got:?}");
    assert_eq!(done + busy, UPLOADS, "only 200 or the busy page: {got:?}");
}
