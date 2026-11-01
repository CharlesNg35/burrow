use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use burrow::config::settings::ServerSettings;
use burrow::wire::secret::Secret;
use bytes::Bytes;
use tokio::net::TcpListener;
use tokio_rustls::rustls::pki_types::ServerName;

use crate::common::*;
use crate::kit::*;
use crate::sites::with_files;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Second {
    Vanish,
    Refuse,
}

async fn h2c_app(second: Second) -> (SocketAddr, Arc<AtomicUsize>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let asked = Arc::new(AtomicUsize::new(0));
    let count = asked.clone();
    let conns = Arc::new(AtomicUsize::new(0));
    tokio::spawn(async move {
        while let Ok((s, _)) = l.accept().await {
            let (count, first_conn) = (count.clone(), conns.fetch_add(1, Ordering::SeqCst) == 0);
            tokio::spawn(async move {
                let Ok(mut h2) = h2::server::handshake(s).await else {
                    return;
                };
                let mut served = 0;
                while let Some(Ok((_req, mut respond))) = h2.accept().await {
                    served += 1;
                    count.fetch_add(1, Ordering::SeqCst);
                    if first_conn && served == 2 {
                        match second {
                            Second::Vanish => return,
                            Second::Refuse => {
                                respond.send_reset(h2::Reason::REFUSED_STREAM);
                                continue;
                            }
                        }
                    }
                    let resp = http::Response::new(());
                    if let Ok(mut body) = respond.send_response(resp, false) {
                        let _ = body.send_data(Bytes::from_static(b"ok"), true);
                    }
                }
            });
        }
    });
    (addr, asked)
}

pub async fn h2_visitor(web: SocketAddr, cert: &Cert) -> h2::client::SendRequest<Bytes> {
    let tcp = tokio::net::TcpStream::connect(web).await.unwrap();
    let name = ServerName::try_from("localhost").unwrap();
    let tls = trusting_alpn(cert, &[b"h2"])
        .connect(name, tcp)
        .await
        .unwrap();
    let (c, conn) = h2::client::handshake(tls).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    c
}

pub async fn status_of(c: &h2::client::SendRequest<Bytes>, xff: Option<&str>) -> u16 {
    let mut c = c.clone().ready().await.unwrap();
    let mut req = http::Request::get("https://localhost/");
    if let Some(ip) = xff {
        req = req.header("x-forwarded-for", ip);
    }
    let (resp, _) = c.send_request(req.body(()).unwrap(), true).unwrap();
    resp.await.map_or(0, |r| r.status().as_u16())
}

pub fn h2_site(app: SocketAddr, dir: &std::path::Path, c: &Cert) -> (ServerSettings, Secret) {
    let secret = Secret::generate();
    let mut s = web_server(&secret, &[("blog", site(&["home"], app), &["localhost"])]);
    with_files(&mut s, "blog", c, dir);
    let web = s.web.as_mut().unwrap();
    web.https = Some(format!("127.0.0.1:{}", free_port()).parse().unwrap());
    web.sites.get_mut("blog").unwrap().options.target_http2 = true;
    (s, secret)
}

async fn second_request(second: Second) -> (u16, usize) {
    let (app, asked) = h2c_app(second).await;
    let dir = tempfile::tempdir().unwrap();
    let c = cert(&["localhost"]);
    let (s, secret) = h2_site(app, dir.path(), &c);
    let t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    let v = h2_visitor(t.server.web_addr.unwrap(), &c).await;
    assert_eq!(status_of(&v, None).await, 200);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let got = status_of(&v, None).await;
    (got, asked.load(Ordering::SeqCst))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_http2_get_lost_with_its_app_connection_is_not_sent_again() {
    let (status, asked) = second_request(Second::Vanish).await;
    assert_eq!(status, 502);
    assert_eq!(asked, 2, "the app may have run it, so it is not sent again");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_http2_get_the_app_refused_is_sent_again_once() {
    let (status, asked) = second_request(Second::Refuse).await;
    assert_eq!(status, 200, "REFUSED_STREAM means it was not processed");
    assert_eq!(asked, 3);
}
