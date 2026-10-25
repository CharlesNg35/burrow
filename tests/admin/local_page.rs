use std::time::Duration;

use burrow::admin_link::open::check_access;
use burrow::admin_local::bridge::UI_CLIENT;
use burrow::wire::secret::Secret;
use serde_json::{Value, json};
use tokio::sync::watch;

use super::common;
use super::{World, world};

struct Local {
    port: u16,
    run: String,
    token: String,
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<Result<(), burrow::base::failure::Failure>>,
}

async fn local(w: &World, name: &str, secret: &Secret) -> Local {
    let mut s = common::visitor_settings(w.addr, secret.clone(), Vec::new());
    s.name = name.to_string();
    let listener = burrow::admin_link::bind(0).await.unwrap();
    let opts = burrow::admin_local::Options {
        port: Some(0),
        open: false,
        print_url: false,
        url_file: None,
        idle_exit: Duration::from_secs(600),
        ui_dir: None,
    };
    let (stop, stop_rx) = watch::channel(false);
    let (url_tx, url_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let mut url_tx = Some(url_tx);
        burrow::admin_local::serve(listener, &s, &opts, stop_rx, |r| {
            if let Some(tx) = url_tx.take() {
                let _ = tx.send(r.url.clone());
            }
            Ok(())
        })
        .await
    });
    let url = url_rx.await.unwrap();
    let port: u16 = url
        .trim_start_matches("http://127.0.0.1:")
        .split('/')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let token = url.split("#token=").nth(1).unwrap().to_string();
    let run = url
        .split("#token=")
        .next()
        .unwrap()
        .trim_start_matches(&format!("http://127.0.0.1:{port}"))
        .trim_end_matches('/')
        .to_string();
    Local {
        port,
        run,
        token,
        stop,
        task,
    }
}

async fn http_get(port: u16, head: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    s.write_all(head.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await;
    String::from_utf8_lossy(&out).into_owned()
}

type Ws =
    tokio_websockets::WebSocketStream<tokio_websockets::MaybeTlsStream<tokio::net::TcpStream>>;

async fn ws(port: u16, run: &str, origin: &str) -> Result<Ws, String> {
    let b = tokio_websockets::ClientBuilder::new()
        .uri(&format!("ws://127.0.0.1:{port}{run}/api"))
        .unwrap()
        .add_header(
            http::header::ORIGIN,
            http::HeaderValue::from_str(origin).unwrap(),
        )
        .unwrap();
    b.connect().await.map(|(w, _)| w).map_err(|e| e.to_string())
}

async fn ws_text(w: &mut Ws) -> Option<Value> {
    use futures_util::StreamExt;
    loop {
        let m = tokio::time::timeout(Duration::from_secs(10), w.next())
            .await
            .ok()??
            .ok()?;
        if m.is_close() {
            return None;
        }
        if let Some(t) = m.as_text() {
            return serde_json::from_str(t).ok();
        }
    }
}

async fn ws_send(w: &mut Ws, v: Value) {
    use futures_util::SinkExt;
    w.send(tokio_websockets::Message::text(v.to_string()))
        .await
        .unwrap();
}

#[tokio::test]
async fn the_local_page_is_only_served_to_this_machine() {
    let w = world().await;
    let l = local(&w, "ops", &w.ops).await;
    let ok = http_get(
        l.port,
        &format!(
            "GET {}/ HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
            l.run, l.port
        ),
    )
    .await;
    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
    assert!(ok.contains("Clear-Site-Data: \"cache\""), "{ok}");
    if !ok.contains("was not built into this binary") {
        assert!(ok.contains(&format!("src=\"{}/", l.run)), "{ok}");
    }
    let root = http_get(
        l.port,
        &format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n", l.port),
    )
    .await;
    assert!(root.starts_with("HTTP/1.1 302"), "{root}");
    assert!(
        root.contains(&format!("Location: {}/\r\n", l.run)),
        "{root}"
    );
    assert!(root.contains("Cache-Control: no-store"), "{root}");
    let api = http_get(
        l.port,
        &format!(
            "GET /api HTTP/1.1\r\nHost: 127.0.0.1:{p}\r\nOrigin: http://127.0.0.1:{p}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
            p = l.port
        ),
    )
    .await;
    assert!(
        api.starts_with("HTTP/1.1 404"),
        "the api answers only under this run's prefix: {api}"
    );
    assert!(ok.contains("Content-Security-Policy: default-src 'none'"));
    assert!(ok.contains("Cache-Control: no-store"));
    let get = |path: String| {
        let port = l.port;
        async move {
            http_get(
                port,
                &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
            )
            .await
        }
    };
    for page in ["/services/api.v2", "/services/a..b", "/services/api"] {
        let got = get(format!("{}{page}", l.run)).await;
        assert!(got.starts_with("HTTP/1.1 200"), "{page}: {got}");
        let moved = get(page.to_string()).await;
        assert!(moved.starts_with("HTTP/1.1 302"), "{page}: {moved}");
        assert!(
            moved.contains(&format!("Location: {}{page}\r\n", l.run)),
            "{page}: {moved}"
        );
    }
    for bad in ["/../x".to_string(), format!("{}/../x", l.run)] {
        let got = get(bad.clone()).await;
        assert!(got.starts_with("HTTP/1.1 400"), "{bad}: {got}");
    }
    for asset in burrow::admin_local::assets::ASSETS {
        let got = http_get(
            l.port,
            &format!(
                "GET {}{} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
                l.run, asset.path, l.port
            ),
        )
        .await;
        let head = got.split("\r\n\r\n").next().unwrap_or_default();
        assert!(head.starts_with("HTTP/1.1 200"), "{} {head}", asset.path);
        assert!(
            head.contains("Cache-Control: no-store") && !head.contains("immutable"),
            "{} may be cached by the browser: {head}",
            asset.path
        );
    }
    assert!(!ok.to_ascii_lowercase().contains("set-cookie"));
    assert!(!ok.to_ascii_lowercase().contains("access-control-allow"));
    let evil = http_get(l.port, "GET / HTTP/1.1\r\nHost: evil.example\r\n\r\n").await;
    assert!(evil.starts_with("HTTP/1.1 403"), "{evil}");
    let rebind = http_get(
        l.port,
        &format!("GET / HTTP/1.1\r\nHost: evil.example:{}\r\n\r\n", l.port),
    )
    .await;
    assert!(rebind.starts_with("HTTP/1.1 403"), "{rebind}");
    let cross = http_get(
        l.port,
        &format!(
            "GET / HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nSec-Fetch-Site: cross-site\r\n\r\n",
            l.port
        ),
    )
    .await;
    assert!(cross.starts_with("HTTP/1.1 403"), "{cross}");
    let bad_origin = ws(l.port, &l.run, "http://evil.example").await;
    assert!(bad_origin.is_err());
    let _ = l.stop.send(true);
    let _ = w.stop.send(true);
}

async fn send_and_read(port: u16, bytes: &[u8], wait: Duration) -> (String, Duration) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let _ = s.write_all(bytes).await;
    let mut out = Vec::new();
    let _ = tokio::time::timeout(wait, s.read_to_end(&mut out)).await;
    (
        String::from_utf8_lossy(&out).into_owned(),
        started.elapsed(),
    )
}

#[tokio::test]
async fn the_local_server_keeps_its_limits_and_closes_after_each_response() {
    let w = world().await;
    let l = local(&w, "ops", &w.ops).await;
    let get = format!(
        "GET {}/ HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
        l.run, l.port
    );
    let (two, took) = send_and_read(
        l.port,
        format!("{get}{get}").as_bytes(),
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(two.matches("HTTP/1.1 200").count(), 1, "{two}");
    assert!(
        took < Duration::from_secs(3),
        "the connection stayed open: {took:?}"
    );
    let big = format!(
        "GET {}/ HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Pad: {}\r\n\r\n",
        l.run,
        l.port,
        "a".repeat(burrow::admin_local::http::MAX_HEAD + 1024)
    );
    let (got, _) = send_and_read(l.port, big.as_bytes(), Duration::from_secs(5)).await;
    assert!(
        got.is_empty() || got.starts_with("HTTP/1.1 431"),
        "an oversized head was served: {got}"
    );
    let cookies = format!(
        "GET {}/ HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nCookie: jupyter={}\r\n\r\n",
        l.run,
        l.port,
        "c".repeat(32 * 1024)
    );
    let (got, _) = send_and_read(l.port, cookies.as_bytes(), Duration::from_secs(5)).await;
    assert!(
        got.starts_with("HTTP/1.1 200"),
        "large localhost cookies locked the browser out: {}",
        got.lines().next().unwrap_or_default()
    );
    let (slow, took) = send_and_read(
        l.port,
        format!("GET {}/ HTTP/1.1\r\nHost: 127.0.0.1", l.run).as_bytes(),
        Duration::from_secs(12),
    )
    .await;
    assert!(
        slow.is_empty() || slow.starts_with("HTTP/1.1 408"),
        "a slow head was served: {slow}"
    );
    assert!(
        took >= Duration::from_secs(4) && took < Duration::from_secs(8),
        "a slow head was cut after {took:?}"
    );
    let mut held = Vec::new();
    for _ in 0..burrow::admin_local::MAX_OPEN {
        use tokio::io::AsyncWriteExt;
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", l.port))
            .await
            .unwrap();
        s.write_all(b"GET / HTTP/1.1\r\n").await.unwrap();
        held.push(s);
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (over, took) = send_and_read(l.port, get.as_bytes(), Duration::from_secs(3)).await;
    assert!(
        over.is_empty(),
        "a connection over the cap was served: {over}"
    );
    assert!(took < Duration::from_secs(2), "{took:?}");
    drop(held);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (again, _) = send_and_read(l.port, get.as_bytes(), Duration::from_secs(5)).await;
    assert!(again.starts_with("HTTP/1.1 200"), "{again}");
    let _ = l.stop.send(true);
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn a_socket_without_the_token_is_dropped_even_if_it_ignores_the_close() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let w = world().await;
    let l = local(&w, "ops", &w.ops).await;
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", l.port))
        .await
        .unwrap();
    let head = format!(
        "GET {run}/api HTTP/1.1\r\nHost: 127.0.0.1:{p}\r\nOrigin: http://127.0.0.1:{p}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
        p = l.port,
        run = l.run
    );
    s.write_all(head.as_bytes()).await.unwrap();
    let started = std::time::Instant::now();
    let mut buf = vec![0u8; 4096];
    let ended = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match s.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    })
    .await;
    assert!(ended.is_ok(), "the bridge kept a silent socket open");
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "dropped about 5 s after the upgrade, took {:?}",
        started.elapsed()
    );
    let _ = l.stop.send(true);
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn the_websocket_needs_the_token_first() {
    let w = world().await;
    let l = local(&w, "ops", &w.ops).await;
    let origin = format!("http://127.0.0.1:{}", l.port);
    let started = std::time::Instant::now();
    let mut silent = ws(l.port, &l.run, &origin).await.unwrap();
    assert!(
        ws_text(&mut silent).await.is_none(),
        "no token, no messages"
    );
    assert!(
        started.elapsed() < Duration::from_secs(7),
        "closed within about 5 s"
    );
    let mut wrong = ws(l.port, &l.run, &origin).await.unwrap();
    ws_send(&mut wrong, json!({"auth": "0".repeat(64)})).await;
    assert!(ws_text(&mut wrong).await.is_none());
    let mut right = ws(l.port, &l.run, &origin).await.unwrap();
    ws_send(&mut right, json!({"auth": l.token})).await;
    let open = ws_text(&mut right).await.unwrap();
    assert_eq!(open["ev"], "bridge");
    assert_eq!(open["body"]["state"], "open");
    assert!(
        open["body"]["server"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "the open event names the server from the visitor config: {open}"
    );
    ws_send(&mut right, json!({"id": 1, "t": "welcome", "body": {}})).await;
    let welcome = ws_text(&mut right).await.unwrap();
    assert_eq!(welcome["re"], 1);
    assert_eq!(welcome["body"]["you"]["visitor"], "ops");
    let big = "x".repeat(150 * 1024);
    ws_send(
        &mut right,
        json!({"id": 2, "t": "ping", "body": {"pad": big}}),
    )
    .await;
    let pong = ws_text(&mut right).await.unwrap();
    assert_eq!(
        pong["re"], 2,
        "a message larger than one record crosses the bridge"
    );
    let _ = w.stop.send(true);
    let mut closed = None;
    for _ in 0..10 {
        match ws_text(&mut right).await {
            Some(v) if v["ev"] == "bridge" => {
                closed = Some(v);
                break;
            }
            Some(_) => {}
            None => break,
        }
    }
    let closed = closed.expect("the bridge says the server side ended");
    assert_eq!(closed["body"]["state"], "server-closed");
    let _ = l.stop.send(true);
    let _ = l.task.await;
}

#[tokio::test]
async fn a_visitor_without_admin_rights_is_told_before_anything_opens() {
    let w = world().await;
    let mut s = common::visitor_settings(w.addr, w.plain.clone(), Vec::new());
    s.name = "plain".into();
    let e = check_access(&s, UI_CLIENT).await.unwrap_err();
    assert_eq!(
        e.message,
        format!("visitor \"plain\" has no admin rights on {}", s.server)
    );
    assert_eq!(
        e.hint.as_deref(),
        Some(
            "add \"admin: write\" (or \"admin: read\") to visitors.plain in server.yaml on the server"
        )
    );
    assert_eq!(e.exit, 1);
    let mut s = common::visitor_settings(w.addr, w.viewer.clone(), Vec::new());
    s.name = "viewer".into();
    check_access(&s, UI_CLIENT).await.unwrap();
    let l = local(&w, "plain", &w.plain).await;
    let origin = format!("http://127.0.0.1:{}", l.port);
    let mut tab = ws(l.port, &l.run, &origin).await.unwrap();
    ws_send(&mut tab, json!({"auth": l.token})).await;
    let refused = ws_text(&mut tab).await.unwrap();
    assert_eq!(refused["body"]["state"], "refused");
    assert_eq!(refused["body"]["reason"], "forbidden");
    let _ = l.stop.send(true);
    let _ = w.stop.send(true);
}
