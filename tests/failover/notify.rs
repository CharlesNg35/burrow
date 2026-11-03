use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use burrow::config::settings::{NotifyTarget, NotifyVia, Service};
use burrow::wire::secret::Secret;
use burrow_config::{NotifyEvent, Protocol};
use http_body_util::BodyExt;
use tokio::net::TcpListener;

use super::common::*;

type Got = Arc<Mutex<Vec<serde_json::Value>>>;

async fn webhook() -> (String, Got) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/burrow", l.local_addr().unwrap());
    let got: Got = Arc::default();
    let log = got.clone();
    tokio::spawn(async move {
        while let Ok((s, _)) = l.accept().await {
            let log = log.clone();
            let service =
                hyper::service::service_fn(move |req: http::Request<hyper::body::Incoming>| {
                    let log = log.clone();
                    async move {
                        let body = req.into_body().collect().await.unwrap().to_bytes();
                        log.lock()
                            .unwrap()
                            .push(serde_json::from_slice(&body).unwrap());
                        let ok = http::Response::new(http_body_util::Empty::<bytes::Bytes>::new());
                        Ok::<_, std::convert::Infallible>(ok)
                    }
                });
            tokio::spawn(
                hyper::server::conn::http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(s), service),
            );
        }
    });
    (url, got)
}

fn free_addr() -> SocketAddr {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

fn events(got: &Got) -> Vec<(String, bool)> {
    got.lock()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["event"].as_str().unwrap_or_default().to_string(),
                v["resolved"].as_bool().unwrap_or_default(),
            )
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_service_that_goes_down_and_back_sends_one_message_each() {
    let (url, got) = webhook().await;
    let target = free_addr();
    let secret = Secret::generate();
    let mut s = server_settings(vec![secret.clone()]);
    s.public_address = Some("tunnel.example.com".into());
    s.notify = vec![NotifyTarget {
        via: NotifyVia::Webhook(url),
        events: [NotifyEvent::SiteDown, NotifyEvent::SiteBack].into(),
    }];
    let checked = Service {
        health: Some(quick_health()),
        ..service(target, Protocol::Tcp)
    };
    let t = tunnel_with(s, vec![("app", checked)], |c| c, secret).await;
    let seen = got.clone();
    wait_long(|| !seen.lock().unwrap().is_empty()).await;
    assert_eq!(events(&got), [("site_down".to_string(), false)]);
    let first = got.lock().unwrap()[0].clone();
    assert_eq!(first["name"], "app");
    assert_eq!(first["title"], "[tunnel.example.com] Service app is down");
    let _up = echo_on(target).await;
    let seen = got.clone();
    wait_long(|| seen.lock().unwrap().len() >= 2).await;
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(
        events(&got),
        [
            ("site_down".to_string(), false),
            ("site_back".to_string(), true)
        ]
    );
    let status = t.server.shared.status();
    assert_eq!(status.notify.len(), 1);
    assert_eq!(
        (status.notify[0].sent, status.notify[0].failing.clone()),
        (2, None)
    );
}
