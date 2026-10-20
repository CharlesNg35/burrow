mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use burrow::config::{Loaded, Role, load_file};
use burrow::dialer::Dialer;
use burrow::dialer::admin::{AdminOpenError, AdminRefusal, AdminRx, AdminTx};
use burrow::secret::Secret;
use burrow::server::Server;
use burrow_admin_proto::frame::AdminOpen;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, watch};

const PING: Duration = Duration::from_secs(1);
const SILENCE: Duration = Duration::from_secs(4);

struct World {
    _dir: tempfile::TempDir,
    addr: SocketAddr,
    ops: Secret,
    stop: watch::Sender<bool>,
}

use common::free_port;

async fn world() -> World {
    world_with(0).await
}

async fn world_with(services: usize) -> World {
    common::logs();
    burrow::admin::transport::shorten_keepalive_for_tests(PING, SILENCE);
    common::more_files();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.yaml");
    let (home, ops) = (Secret::generate(), Secret::generate());
    let listener = if common::ws_mode() {
        format!(
            "websocket:\n  listen: 127.0.0.1:{}\n  path: /burrow\n",
            free_port()
        )
    } else {
        format!("listen: 127.0.0.1:{}\n", free_port())
    };
    let mut text = format!(
        "{listener}clients:\n  home:\n    secret: {}\nvisitors:\n  ops:\n    secret: {}\n    admin: write\n  laptop:\n    secret: {}\n",
        home.encode(),
        ops.encode(),
        Secret::generate().encode(),
    );
    if services > 0 {
        text.push_str("services:\n");
        for i in 0..services {
            text.push_str(&format!(
                "  service-{i}: {{ client: home, target: \"127.0.0.1:{}\", visitors: [laptop] }}\n",
                10_000 + i
            ));
        }
    }
    burrow::instance::write_private(&path, &text).unwrap();
    let env = burrow::run::Env::default();
    let outcome = load_file(&path, Some(Role::Server), &env).unwrap();
    let Some(Loaded::Server(settings, _)) = outcome.loaded else {
        panic!("server config")
    };
    let server = Server::bind(*settings.clone()).await.unwrap();
    let addr = server.local_addr().or(server.websocket_addr()).unwrap();
    let shared = server.shared();
    let (hub, nudges) = burrow::admin::adapters::hub(&shared, &path, env.clone());
    shared.set_admin(hub);
    let (reload_tx, reload_rx) = mpsc::channel(4);
    tokio::spawn(burrow::reload::follow_nudged(
        path.clone(),
        *settings,
        env,
        burrow::reload::accept(),
        reload_tx,
        Some(nudges),
    ));
    let (stop, stop_rx) = watch::channel(false);
    tokio::spawn(server.run(reload_rx, stop_rx));
    World {
        _dir: dir,
        addr,
        ops,
        stop,
    }
}

async fn relay(to: SocketAddr) -> (SocketAddr, Arc<AtomicBool>) {
    let frozen = Arc::new(AtomicBool::new(false));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let f = frozen.clone();
    tokio::spawn(async move {
        while let Ok((inbound, _)) = l.accept().await {
            let Ok(outbound) = tokio::net::TcpStream::connect(to).await else {
                continue;
            };
            let (ri, wi) = inbound.into_split();
            let (ro, wo) = outbound.into_split();
            for (mut r, mut w) in [
                (
                    Box::new(ri) as Box<dyn tokio::io::AsyncRead + Unpin + Send>,
                    Box::new(wo) as Box<dyn tokio::io::AsyncWrite + Unpin + Send>,
                ),
                (Box::new(ro), Box::new(wi)),
            ] {
                let f = f.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 16 * 1024];
                    loop {
                        let n = match r.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => n,
                        };
                        while f.load(Ordering::SeqCst) {
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                        if w.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                    let _ = w.shutdown().await;
                });
            }
        }
    });
    (addr, frozen)
}

fn open_msg() -> AdminOpen {
    AdminOpen {
        min: 1,
        max: 1,
        client: "tests/admin_keepalive".into(),
        features: Vec::new(),
    }
}

#[tokio::test]
async fn the_server_ends_a_subscribed_session_whose_caller_went_silent() {
    let w = world().await;
    let mut s = common::visitor_settings(w.addr, w.ops.clone(), Vec::new());
    s.name = "ops".into();
    let (mut rx, mut tx, _) = Dialer::visitor(&s).open_admin(&open_msg()).await.unwrap();
    let sub = json!({"id": 1, "t": "subscribe", "body": {"status_ms": 500, "config": true}});
    tx.send(sub.to_string().as_bytes()).await.unwrap();
    let started = Instant::now();
    let mut statuses = 0;
    let end = tokio::time::timeout(SILENCE * 4, async {
        while let Ok(Some(m)) = rx.recv().await {
            if String::from_utf8_lossy(&m).contains("\"ev\":\"status\"") {
                statuses += 1;
            }
        }
    })
    .await;
    assert!(
        end.is_ok(),
        "the session was still open after {:?} of silence with {statuses} status events",
        started.elapsed()
    );
    assert!(started.elapsed() >= SILENCE - Duration::from_millis(500));
    assert!(
        statuses >= 4,
        "status kept coming until the end: {statuses}"
    );
    let _ = w.stop.send(true);
}

async fn admin(w: &World) -> Result<(AdminRx, AdminTx), AdminOpenError> {
    let mut s = common::visitor_settings(w.addr, w.ops.clone(), Vec::new());
    s.name = "ops".into();
    let (rx, tx, _) = Dialer::visitor(&s).open_admin(&open_msg()).await?;
    Ok((rx, tx))
}

#[tokio::test]
async fn a_caller_that_stops_reading_gives_its_seat_back() {
    let w = world_with(1000).await;
    let (_unread, mut tx) = admin(&w).await.unwrap();
    let sub = json!({"id": 1, "t": "subscribe", "body": {"status_ms": 500, "config": true}});
    tx.send(sub.to_string().as_bytes()).await.unwrap();
    for id in 2..300 {
        let req = json!({"id": id, "t": "state", "body": {}});
        tx.send(req.to_string().as_bytes()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let started = Instant::now();
    let (_second_rx, mut second) = admin(&w).await.unwrap();
    let alive = tokio::spawn(async move {
        loop {
            if second.ping().await.is_err() {
                break;
            }
            tokio::time::sleep(PING).await;
        }
    });
    let mut refused = 0;
    let third = loop {
        match admin(&w).await {
            Ok(s) => break s,
            Err(AdminOpenError::Refused(AdminRefusal::Busy)) => refused += 1,
            Err(e) => panic!("{e:?}"),
        }
        assert!(
            started.elapsed() < SILENCE * 5,
            "the seat of a caller that stopped reading is still held after {:?}",
            started.elapsed()
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    assert!(
        refused > 0,
        "the seat was free at once, so the silent caller did not hold it"
    );
    drop(third);
    alive.abort();
    let _ = w.stop.send(true);
}

struct Local {
    port: u16,
    run: String,
    token: String,
    stop: watch::Sender<bool>,
}

async fn local(server: SocketAddr, secret: &Secret) -> Local {
    let mut s = common::visitor_settings(server, secret.clone(), Vec::new());
    s.name = "ops".into();
    let listener = burrow::admin_local::bind(0).await.unwrap();
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
    tokio::spawn(async move {
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
    }
}

type Ws =
    tokio_websockets::WebSocketStream<tokio_websockets::MaybeTlsStream<tokio::net::TcpStream>>;

async fn dashboard(l: &Local) -> Ws {
    let origin = format!("http://127.0.0.1:{}", l.port);
    let (mut ws, _) = tokio_websockets::ClientBuilder::new()
        .uri(&format!("ws://127.0.0.1:{}{}/api", l.port, l.run))
        .unwrap()
        .add_header(
            http::header::ORIGIN,
            http::HeaderValue::from_str(&origin).unwrap(),
        )
        .unwrap()
        .connect()
        .await
        .unwrap();
    let send = |v: Value| tokio_websockets::Message::text(v.to_string());
    ws.send(send(json!({"auth": l.token}))).await.unwrap();
    let open = next_text(&mut ws, Duration::from_secs(10)).await.unwrap();
    assert_eq!(open["body"]["state"], "open", "{open}");
    ws.send(send(
        json!({"id": 1, "t": "subscribe", "body": {"status_ms": 500, "config": true}}),
    ))
    .await
    .unwrap();
    ws
}

async fn next_text(ws: &mut Ws, within: Duration) -> Option<Value> {
    loop {
        let m = tokio::time::timeout(within, ws.next()).await.ok()??.ok()?;
        if m.is_close() {
            return None;
        }
        if let Some(t) = m.as_text() {
            return serde_json::from_str(t).ok();
        }
    }
}

#[tokio::test]
async fn an_open_dashboard_stays_connected() {
    let w = world().await;
    let l = local(w.addr, &w.ops).await;
    let mut ws = dashboard(&l).await;
    let started = Instant::now();
    let mut statuses = 0;
    while started.elapsed() < SILENCE * 4 {
        let v = next_text(&mut ws, SILENCE)
            .await
            .unwrap_or_else(|| panic!("closed after {:?}", started.elapsed()));
        assert_ne!(v["ev"], "bridge", "{v} after {:?}", started.elapsed());
        if v["ev"] == "status" {
            statuses += 1;
        }
    }
    assert!(statuses >= 20, "{statuses}");
    let _ = l.stop.send(true);
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn the_bridge_tells_the_page_when_the_server_went_silent() {
    let w = world().await;
    let (via, frozen) = relay(w.addr).await;
    let l = local(via, &w.ops).await;
    let mut ws = dashboard(&l).await;
    let first = next_text(&mut ws, Duration::from_secs(10)).await;
    assert!(first.is_some());
    frozen.store(true, Ordering::SeqCst);
    let started = Instant::now();
    let closed = loop {
        match next_text(&mut ws, SILENCE * 3).await {
            None => break None,
            Some(v) if v["ev"] == "bridge" => break Some(v),
            Some(_) => {}
        }
    };
    let closed = closed.unwrap_or_else(|| {
        panic!(
            "no server-closed event within {:?} of server silence",
            started.elapsed()
        )
    });
    assert_eq!(closed["body"]["state"], "server-closed", "{closed}");
    assert!(started.elapsed() < SILENCE * 2, "{:?}", started.elapsed());
    let _ = l.stop.send(true);
    let _ = w.stop.send(true);
}
