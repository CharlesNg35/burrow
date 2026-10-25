use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use burrow::config::settings::{Health, HttpCheck, LogFormat, ServerSettings, Service, TargetTls};
use burrow::wire::secret::Secret;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::common::*;
use crate::kit::*;

type Answer = Arc<dyn Fn(&str) -> String + Send + Sync>;

pub struct App {
    pub addr: SocketAddr,
    pub heard: Arc<Mutex<Vec<String>>>,
    pub answer: Arc<Mutex<Answer>>,
}

pub fn status(line: &str) -> Answer {
    let reply = format!("HTTP/1.1 {line}\r\nContent-Length: 0\r\n\r\n");
    Arc::new(move |_| reply.clone())
}

async fn head_of(s: &mut tokio::net::TcpStream) -> Option<String> {
    let mut head = Vec::new();
    let mut one = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match s.read(&mut one).await {
            Ok(1) => head.push(one[0]),
            _ => return None,
        }
    }
    Some(String::from_utf8_lossy(&head).to_ascii_lowercase())
}

pub async fn app(answer: Answer) -> App {
    app_on("127.0.0.1:0".parse().unwrap(), answer).await
}

pub async fn app_on(addr: SocketAddr, answer: Answer) -> App {
    let l = TcpListener::bind(addr).await.unwrap();
    let addr = l.local_addr().unwrap();
    let heard = Arc::new(Mutex::new(Vec::new()));
    let answer = Arc::new(Mutex::new(answer));
    let (log, current) = (heard.clone(), answer.clone());
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            let (log, current) = (log.clone(), current.clone());
            tokio::spawn(async move {
                while let Some(head) = head_of(&mut s).await {
                    let reply = (current.lock().unwrap().clone())(&head);
                    log.lock().unwrap().push(head);
                    if s.write_all(reply.as_bytes()).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    App {
        addr,
        heard,
        answer,
    }
}

impl App {
    pub fn checks(&self) -> Vec<String> {
        self.heard
            .lock()
            .unwrap()
            .iter()
            .filter(|h| h.contains("user-agent: burrow-health-check/"))
            .cloned()
            .collect()
    }

    pub fn set(&self, answer: Answer) {
        *self.answer.lock().unwrap() = answer;
    }
}

pub fn http_health(path: &str, host: &str) -> Option<Health> {
    Some(Health {
        interval: Duration::from_millis(200),
        timeout: Duration::from_secs(1),
        failures: 1,
        http: Some(HttpCheck {
            path: path.into(),
            host: host.into(),
        }),
    })
}

fn checked(target: SocketAddr, path: &str) -> Service {
    Service {
        health: http_health(path, "blog.example.com"),
        ..site(&["home"], target)
    }
}

fn server(secret: &Secret, svc: Service) -> ServerSettings {
    plain_server(
        secret,
        &[("blog", svc, &["blog.example.com", "www.example.com"])],
    )
}

fn health_of(t: &Tunnel) -> (bool, Option<String>) {
    let s = t.server.shared.status();
    let h = &s.services[0].health[0];
    (h.up, h.reason.clone())
}

async fn wait_health(t: &Tunnel, up: bool) -> Option<String> {
    let mut last = (true, None);
    for _ in 0..100 {
        if t.server.shared.status().services[0].health.len() == 1 {
            last = health_of(t);
            if last.0 == up {
                return last.1;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!(
        "the client never became {}: {last:?}",
        if up { "up" } else { "down" }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_that_answers_400_to_other_hosts_is_checked_with_the_sites_name_and_stays_up() {
    let answer: Answer = Arc::new(|head: &str| {
        let line = if head.contains("\r\nhost: blog.example.com\r\n") {
            "200 OK"
        } else {
            "400 Bad Request"
        };
        format!("HTTP/1.1 {line}\r\nContent-Length: 0\r\n\r\n")
    });
    let app = app(answer).await;
    let secret = Secret::generate();
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("access.log");
    let mut s = server(&secret, checked(app.addr, "/healthz"));
    s.web.as_mut().unwrap().access_log = Some((log.clone(), LogFormat::Common));
    let t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    wait_for(|| app.checks().len() >= 3).await;
    assert_eq!(wait_health(&t, true).await, None);
    let head = &app.checks()[0];
    assert!(head.starts_with("get /healthz http/1.1\r\n"), "{head}");
    assert!(
        head.contains(&format!(
            "\r\nuser-agent: burrow-health-check/{}\r\n",
            env!("CARGO_PKG_VERSION")
        )),
        "{head}"
    );
    assert!(head.contains("\r\naccept: */*\r\n"), "{head}");
    assert!(!head.contains("x-forwarded"), "{head}");
    let s = t.server.shared.status();
    assert_eq!(s.services[0].total, 0);
    assert_eq!(s.services[0].http.as_ref().unwrap().responses, [0; 5]);
    let logged = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(logged.is_empty(), "{logged}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_redirect_is_up_and_not_followed_and_a_login_is_down_with_its_reason() {
    let app = app(status("302 Found\r\nLocation: /elsewhere")).await;
    let secret = Secret::generate();
    let t = tunnel_with(
        server(&secret, checked(app.addr, "/")),
        Vec::new(),
        |c| c,
        secret,
    )
    .await;
    wait_for(|| app.checks().len() >= 3).await;
    assert_eq!(wait_health(&t, true).await, None);
    assert!(
        app.heard
            .lock()
            .unwrap()
            .iter()
            .all(|h| h.starts_with("get / "))
    );
    app.set(status("401 Unauthorized"));
    let why = wait_health(&t, false).await;
    assert_eq!(why.as_deref(), Some("answered 401 Unauthorized"));
    let s = t.server.shared.status();
    assert!(
        s.services[0].set_aside.is_empty(),
        "a reachable app is not set aside"
    );
    app.set(status("200 OK"));
    assert_eq!(wait_health(&t, true).await, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_that_never_answers_is_down_after_the_timeout() {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((s, _)) = l.accept().await {
            held.push(s);
        }
    });
    let secret = Secret::generate();
    let t = tunnel_with(
        server(&secret, checked(addr, "/")),
        Vec::new(),
        |c| c,
        secret,
    )
    .await;
    let why = wait_health(&t, false).await;
    assert_eq!(why.as_deref(), Some("no answer within 1s"));
}

async fn tls_app(c: &Cert) -> SocketAddr {
    let acceptor = acceptor(c);
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((s, _)) = l.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(s).await else {
                    return;
                };
                let mut buf = vec![0u8; 1024];
                let _ = tls.read(&mut buf).await;
                let _ = tls
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                    .await;
                let _ = tls.flush().await;
            });
        }
    });
    addr
}

fn with_ca(s: &mut ServerSettings, ca: &std::path::Path) {
    let site = s.web.as_mut().unwrap().sites.get_mut("blog").unwrap();
    site.options.target_tls = TargetTls::Ca(ca.to_path_buf());
    site.options.target_name = "app.lan".into();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tls_app_is_checked_with_its_ca_and_a_wrong_ca_is_down_with_the_tls_reason() {
    let own = cert(&["app.lan"]);
    let app = tls_app(&own).await;
    let dir = tempfile::tempdir().unwrap();
    let (right, wrong) = (dir.path().join("right.pem"), dir.path().join("wrong.pem"));
    std::fs::write(&right, &own.cert_pem).unwrap();
    std::fs::write(&wrong, cert(&["app.lan"]).cert_pem).unwrap();
    let secret = Secret::generate();
    let mut s = server(&secret, checked(app, "/"));
    with_ca(&mut s, &right);
    let t = tunnel_with(s.clone(), Vec::new(), |c| c, secret).await;
    assert_eq!(wait_health(&t, true).await, None);
    with_ca(&mut s, &wrong);
    t.server.reload(s).await;
    let why = wait_health(&t, false).await.unwrap();
    assert!(
        why.starts_with("TLS to the app failed: invalid peer certificate"),
        "{why}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_proxy_protocol_service_sends_the_local_header_before_the_request() {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let firsts = Arc::new(Mutex::new(Vec::new()));
    let log = firsts.clone();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            let log = log.clone();
            tokio::spawn(async move {
                let mut got = vec![0u8; 512];
                let Ok(n) = s.read(&mut got).await else {
                    return;
                };
                got.truncate(n);
                log.lock().unwrap().push(got);
                let _ = s
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                    .await;
            });
        }
    });
    let secret = Secret::generate();
    let svc = Service {
        proxy_protocol: Some(burrow_config::ProxyVersion::V1),
        ..checked(addr, "/")
    };
    let t = tunnel_with(server(&secret, svc), Vec::new(), |c| c, secret).await;
    wait_for(|| !firsts.lock().unwrap().is_empty()).await;
    let first = String::from_utf8_lossy(&firsts.lock().unwrap()[0]).into_owned();
    assert!(first.starts_with("PROXY UNKNOWN\r\n"), "{first}");
    assert_eq!(wait_health(&t, true).await, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_path_on_reload_starts_the_check_afresh() {
    let answer: Answer = Arc::new(|head: &str| {
        let line = if head.starts_with("get /ready ") {
            "200 OK"
        } else {
            "404 Not Found"
        };
        format!("HTTP/1.1 {line}\r\nContent-Length: 0\r\n\r\n")
    });
    let app = app(answer).await;
    let secret = Secret::generate();
    let mut slow = checked(app.addr, "/old");
    if let Some(h) = slow.health.as_mut() {
        h.interval = Duration::from_secs(3600);
    }
    let s = server(&secret, slow.clone());
    let t = tunnel_with(s.clone(), Vec::new(), |c| c, secret).await;
    let why = wait_health(&t, false).await;
    assert_eq!(why.as_deref(), Some("answered 404 Not Found"));
    let mut next = s;
    let svc = next.services.get_mut("blog").unwrap();
    svc.health.as_mut().unwrap().http.as_mut().unwrap().path = "/ready".into();
    t.server.reload(next).await;
    assert_eq!(wait_health(&t, true).await, None);
    assert!(app.checks().iter().any(|h| h.starts_with("get /ready ")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_site_served_by_self_is_checked_over_http_too() {
    let app = app(status("200 OK")).await;
    let secret = Secret::generate();
    let svc = Service {
        clients: vec!["self".into()],
        ..checked(app.addr, "/healthz")
    };
    let t = tunnel_with(server(&secret, svc), Vec::new(), |c| c, secret).await;
    wait_for(|| !app.checks().is_empty()).await;
    assert_eq!(wait_health(&t, true).await, None);
    assert!(app.checks()[0].contains("\r\nhost: blog.example.com\r\n"));
    app.set(status("503 Service Unavailable"));
    let why = wait_health(&t, false).await;
    assert_eq!(why.as_deref(), Some("answered 503 Service Unavailable"));
}
