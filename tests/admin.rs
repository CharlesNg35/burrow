mod common;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use burrow::config::{Loaded, Role, load_file};
use burrow::dialer::Dialer;
use burrow::dialer::admin::{AdminOpenError, AdminRefusal, AdminRx, AdminTx};
use burrow::secret::Secret;
use burrow::server::Server;
use burrow_admin_proto::frame::AdminOpen;
use burrow_admin_proto::msg::{ErrorBody, ErrorCode, Incoming};
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};

struct World {
    _dir: tempfile::TempDir,
    path: PathBuf,
    addr: SocketAddr,
    home: Secret,
    ops: Secret,
    viewer: Secret,
    plain: Secret,
    stop: watch::Sender<bool>,
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn listener_line() -> String {
    if common::ws_mode() {
        format!(
            "websocket:\n  listen: 127.0.0.1:{}\n  path: /burrow\n",
            free_port()
        )
    } else {
        format!("listen: 127.0.0.1:{} # tcp\n", free_port())
    }
}

async fn world() -> World {
    common::more_files();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.yaml");
    let (home, ops, viewer, plain) = (
        Secret::generate(),
        Secret::generate(),
        Secret::generate(),
        Secret::generate(),
    );
    let text = format!(
        "# admin test server\n{}\nclients:\n  # the home box\n  home:\n    secret: {}\n\nvisitors:\n  ops:\n    secret: {}\n    admin: write\n  viewer:\n    secret: {}\n    admin: read\n  plain:\n    secret: {}\n    reach: {{ home: any }}\n\nservices:\n  web: {{ client: home, bind: 127.0.0.1:{}, target: 3000 }} # keep\n",
        listener_line(),
        home.encode(),
        ops.encode(),
        viewer.encode(),
        plain.encode(),
        free_port()
    );
    burrow::instance::write_private(&path, &text).unwrap();
    let env = burrow::run::Env::default();
    let outcome = load_file(&path, Some(Role::Server), &env).unwrap();
    let report = outcome
        .report
        .render_text(burrow::diagnostics::Style { color: false });
    let Loaded::Server(settings, _) = outcome.loaded.unwrap_or_else(|| panic!("{report}")) else {
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
        path,
        addr,
        home,
        ops,
        viewer,
        plain,
        stop,
    }
}

struct Admin {
    rx: AdminRx,
    tx: AdminTx,
    next: u32,
    events: Vec<(String, Value)>,
}

fn open_msg(min: u8, max: u8) -> AdminOpen {
    AdminOpen {
        min,
        max,
        client: "tests/admin".into(),
        features: Vec::new(),
    }
}

async fn connect(
    w: &World,
    name: &str,
    secret: &Secret,
    min: u8,
    max: u8,
) -> Result<Admin, AdminOpenError> {
    let mut s = common::visitor_settings(w.addr, secret.clone(), Vec::new());
    s.name = name.to_string();
    let (rx, tx, _) = Dialer::visitor(&s).open_admin(&open_msg(min, max)).await?;
    Ok(Admin {
        rx,
        tx,
        next: 0,
        events: Vec::new(),
    })
}

impl Admin {
    async fn send_raw(&mut self, t: &str, body: Value) -> u32 {
        self.next += 1;
        let msg = json!({"id": self.next, "t": t, "body": body});
        self.tx.send(msg.to_string().as_bytes()).await.unwrap();
        self.next
    }

    async fn incoming(&mut self) -> Option<Incoming> {
        let m = tokio::time::timeout(Duration::from_secs(20), self.rx.recv())
            .await
            .expect("an answer in time")
            .ok()??;
        Some(Incoming::parse(std::str::from_utf8(&m).unwrap()).expect("a valid envelope"))
    }

    async fn call(&mut self, t: &str, body: Value) -> Result<Value, ErrorBody> {
        let id = self.send_raw(t, body).await;
        loop {
            match self.incoming().await.expect("the session stays open") {
                Incoming::Reply { re, result } if re == id => return result,
                Incoming::Reply { .. } => {}
                Incoming::Event { ev, body } => self.events.push((ev, body)),
            }
        }
    }

    async fn event(&mut self, name: &str) -> Value {
        if let Some(i) = self.events.iter().position(|(e, _)| e == name) {
            return self.events.remove(i).1;
        }
        loop {
            match self.incoming().await.expect("the session stays open") {
                Incoming::Event { ev, body } if ev == name => return body,
                Incoming::Event { ev, body } => self.events.push((ev, body)),
                Incoming::Reply { .. } => {}
            }
        }
    }

    async fn state(&mut self) -> Value {
        self.call("state", json!({})).await.unwrap()
    }

    async fn plan(&mut self, ops: Value) -> Result<Value, ErrorBody> {
        let rev = self.state().await["rev"].clone();
        self.call("validate", json!({"base": rev, "ops": ops}))
            .await
    }

    async fn apply(&mut self, ops: Value, confirm: &[&str]) -> Result<Value, ErrorBody> {
        let plan = self.plan(ops.clone()).await?;
        self.call(
            "apply",
            json!({"base": plan["base"], "ops": ops, "plan": plan["plan"], "confirm": confirm}),
        )
        .await
    }
}

#[tokio::test]
async fn an_admin_gets_welcome_schema_state_and_live_status() {
    let w = world().await;
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    let welcome = a.call("welcome", json!({})).await.unwrap();
    assert_eq!(welcome["you"]["visitor"], "ops");
    assert_eq!(welcome["you"]["level"], "write");
    assert_eq!(welcome["protocol"], 1);
    assert_eq!(
        welcome["server"]["name_hint"], "",
        "without public_address there is no name hint, never the host name"
    );
    assert!(
        welcome["features"]
            .as_array()
            .unwrap()
            .contains(&json!("history"))
    );
    let schema = a.call("schema", json!({"have": ""})).await.unwrap();
    assert_eq!(schema["hash"], welcome["schema_hash"]);
    assert_eq!(schema["schema"]["schema_version"], 1);
    let same = a
        .call("schema", json!({"have": welcome["schema_hash"]}))
        .await
        .unwrap();
    assert_eq!(same["unchanged"], true);
    let state = a.state().await;
    assert!(state["rev"].as_str().unwrap().starts_with("b2s:"));
    assert_eq!(
        state["values"]["visitors"]["ops"]["secret"]["$secret"]["fp"],
        w.ops.fingerprint()
    );
    assert!(!state.to_string().contains(&w.ops.encode()));
    assert!(state["file"]["writable"].as_bool().unwrap());
    a.call("subscribe", json!({"status_ms": 500}))
        .await
        .unwrap();
    let first = a.event("status").await;
    let second = a.event("status").await;
    assert_eq!(first["clients"][0]["name"], "home");
    assert!(second["uptime_secs"].is_u64());
    let unknown = a.call("teleport", json!({})).await.unwrap_err();
    assert_eq!(unknown.code, ErrorCode::UnknownRequest);
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn only_admin_visitors_get_in_and_sessions_are_capped() {
    let w = world().await;
    let refused = connect(&w, "plain", &w.plain, 1, 1).await.err().unwrap();
    assert!(
        matches!(refused, AdminOpenError::Refused(AdminRefusal::Forbidden)),
        "{refused:?}"
    );
    let wrong = connect(&w, "ops", &w.plain, 1, 1).await.err().unwrap();
    assert!(matches!(wrong, AdminOpenError::Dial(_)), "{wrong:?}");
    let old = connect(&w, "ops", &w.ops, 2, 3).await.err().unwrap();
    match old {
        AdminOpenError::Refused(AdminRefusal::Explained(t)) => {
            assert!(t.contains("versions 1 to 1"), "{t}")
        }
        other => panic!("{other:?}"),
    }
    let a = connect(&w, "viewer", &w.viewer, 1, 1).await.unwrap();
    let b = connect(&w, "viewer", &w.viewer, 1, 1).await.unwrap();
    let busy = connect(&w, "viewer", &w.viewer, 1, 1).await.err().unwrap();
    assert!(
        matches!(busy, AdminOpenError::Refused(AdminRefusal::Busy)),
        "{busy:?}"
    );
    drop((a, b));
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn read_access_cannot_change_anything() {
    let w = world().await;
    let mut v = connect(&w, "viewer", &w.viewer, 1, 1).await.unwrap();
    let state = v.state().await;
    assert_eq!(state["file"]["writable"], false);
    assert_eq!(state["file"]["why_not"]["code"], "read-level");
    let e = v.plan(json!([])).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::Forbidden);
    let e = v.call("generate_secret", json!({})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::Forbidden);
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn validation_errors_name_their_fields() {
    let w = world().await;
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    let state = a.state().await;
    let bind = state["values"]["services"]["web"]["bind"].clone();
    let plan = a
        .plan(json!([{"op": "add", "path": ["services", "api"], "value": {"client": "home", "bind": bind, "target": 3001}}]))
        .await
        .unwrap();
    assert_eq!(plan["valid"], false);
    let d = plan["diags"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["code"] == "duplicate-bind")
        .expect("duplicate-bind")
        .clone();
    let mut paths = vec![d["path"].clone()];
    paths.extend(d["also"].as_array().unwrap().iter().cloned());
    assert!(paths.contains(&json!(["services", "api", "bind"])), "{d}");
    assert!(paths.contains(&json!(["services", "web", "bind"])), "{d}");
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn an_applied_change_keeps_comments_reloads_and_serves() {
    let w = world().await;
    let echo = common::echo_server().await;
    let mut client = common::client_settings(w.addr, w.home.clone());
    client.allow = burrow::allowlist::AllowList::Rules(vec!["127.0.0.0/8".parse().unwrap()]);
    let mut running = common::start_client(client).await;
    common::wait_state(&mut running.state, burrow::client::State::Connected).await;
    let before = std::fs::read_to_string(&w.path).unwrap();
    let port = free_port();
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    let applied = a
        .apply(
            json!([{"op": "add", "path": ["services", "echo"], "value": {"client": "home", "bind": format!("127.0.0.1:{port}"), "target": echo.to_string()}}]),
            &[],
        )
        .await
        .unwrap();
    assert!(applied["rev"].as_str().unwrap().starts_with("b2s:"));
    let after = std::fs::read_to_string(&w.path).unwrap();
    assert!(
        after.starts_with(&before),
        "the old text stays byte for byte:\n{after}"
    );
    assert!(after.contains("# the home box") && after.contains("} # keep"));
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let mut ok = false;
    for _ in 0..100 {
        if common::try_roundtrip(addr, b"hello").await.as_deref() == Some(b"hello".as_slice()) {
            ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(ok, "the new service serves through the tunnel");
    let history = a.call("history", json!({})).await.unwrap();
    assert_eq!(history["entries"][0]["visitor"], "ops");
    assert!(
        history["entries"][0]["changes"][0]["summary"]
            .as_str()
            .unwrap()
            .contains("echo")
    );
    let _ = running.stop.send(true);
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn a_hand_edit_in_between_is_a_conflict() {
    let w = world().await;
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    let ops = json!([{"op": "set", "path": ["services", "web", "target"], "value": 3002}]);
    let plan = a.plan(ops.clone()).await.unwrap();
    let text = std::fs::read_to_string(&w.path).unwrap();
    let other = Secret::generate();
    let edited = text.replace(
        "\nvisitors:",
        &format!("  office:\n    secret: {}\n\nvisitors:", other.encode()),
    );
    std::fs::write(&w.path, &edited).unwrap();
    let e = a
        .call(
            "apply",
            json!({"base": plan["base"], "ops": ops, "plan": plan["plan"], "confirm": []}),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Conflict);
    assert_ne!(e.body["current"], plan["base"]);
    assert_eq!(
        std::fs::read_to_string(&w.path).unwrap(),
        edited,
        "nothing is written on a conflict"
    );
    a.apply(ops, &[]).await.unwrap();
    let last = std::fs::read_to_string(&w.path).unwrap();
    assert!(last.contains("office:") && last.contains("target: 3002"));
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn a_crafted_change_of_a_locked_field_is_refused_and_nothing_is_written() {
    let w = world().await;
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    let before = std::fs::read_to_string(&w.path).unwrap();
    for ops in [
        json!([{"op": "set", "path": ["listen"], "value": 7999}]),
        json!([{"op": "set", "path": ["insecure_plaintext"], "value": true}]),
        json!([{"op": "add", "path": ["websocket"], "value": {"listen": 7998}}]),
    ] {
        let e = a.plan(ops.clone()).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::LockedField, "{ops}");
        let rev = a.state().await["rev"].clone();
        let e = a
            .call(
                "apply",
                json!({"base": rev, "ops": ops, "plan": "b2s:0", "confirm": []}),
            )
            .await
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::LockedField);
    }
    assert_eq!(std::fs::read_to_string(&w.path).unwrap(), before);
    let schema = a.call("schema", json!({})).await.unwrap();
    assert_eq!(
        schema["schema"]["root"]["sem"]["fields"][0]["node"]["locked"],
        true
    );
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn the_guard_refuses_every_lockout() {
    let w = world().await;
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    let before = std::fs::read_to_string(&w.path).unwrap();
    let cases = [
        (
            json!([{"op": "remove", "path": ["visitors", "ops", "admin"]}]),
            "self-demoted",
        ),
        (
            json!([{"op": "remove", "path": ["visitors", "ops"]}]),
            "self-removed",
        ),
        (
            json!([{"op": "set", "path": ["visitors", "ops", "admin_from"], "value": ["10.0.0.0/8"]}]),
            "self-source",
        ),
        (
            json!([{"op": "set", "path": ["visitors", "ops", "admin"], "value": "read"}]),
            "self-demoted",
        ),
    ];
    for (ops, code) in cases {
        let plan = a.plan(ops.clone()).await.unwrap();
        assert!(
            plan["refusals"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["code"] == code),
            "{code}: {plan}"
        );
        let e = a
            .call(
                "apply",
                json!({"base": plan["base"], "ops": ops, "plan": plan["plan"], "confirm": ["ops"]}),
            )
            .await
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::LockedOut, "{code}");
    }
    assert_eq!(std::fs::read_to_string(&w.path).unwrap(), before);
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn losing_admin_rights_ends_the_session() {
    let w = world().await;
    let mut v = connect(&w, "viewer", &w.viewer, 1, 1).await.unwrap();
    v.call("subscribe", json!({"config": true})).await.unwrap();
    let text = std::fs::read_to_string(&w.path).unwrap();
    std::fs::write(&w.path, text.replace("    admin: read\n", "")).unwrap();
    let body = v.event("revoked").await;
    assert_eq!(body["reason"], "admin removed");
    let closed = tokio::time::timeout(Duration::from_secs(5), v.rx.recv())
        .await
        .unwrap();
    assert!(matches!(closed, Ok(None) | Err(_)));
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn a_changed_admin_level_ends_the_session() {
    let w = world().await;
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    a.call("subscribe", json!({"config": true})).await.unwrap();
    let mut v = connect(&w, "viewer", &w.viewer, 1, 1).await.unwrap();
    v.call("subscribe", json!({"config": true})).await.unwrap();
    let text = std::fs::read_to_string(&w.path).unwrap();
    let swapped = text
        .replace("    admin: write\n", "    admin: WAS_WRITE\n")
        .replace("    admin: read\n", "    admin: write\n")
        .replace("    admin: WAS_WRITE\n", "    admin: read\n");
    std::fs::write(&w.path, swapped).unwrap();
    for s in [&mut a, &mut v] {
        let body = s.event("revoked").await;
        assert_eq!(body["reason"], "admin level changed");
        let closed = tokio::time::timeout(Duration::from_secs(5), s.rx.recv())
            .await
            .unwrap();
        assert!(matches!(closed, Ok(None) | Err(_)), "{closed:?}");
    }
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    let err = a
        .plan(json!([{"op": "set", "path": ["visitors", "ops", "admin"], "value": "write"}]))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden, "{err:?}");
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn an_oversized_message_closes_the_session() {
    let w = world().await;
    let mut a = connect(&w, "ops", &w.ops, 1, 1).await.unwrap();
    a.call("ping", json!({})).await.unwrap();
    let big = vec![b'x'; 4 * 1024 * 1024 + 1];
    assert!(
        a.tx.send(&big).await.is_err(),
        "the client refuses to send it"
    );
    let chunk = vec![b'x'; burrow_admin_proto::frame::MAX_CHUNK];
    let mut sent = 0;
    while sent <= 4 * 1024 * 1024 {
        let mut rec = vec![burrow_admin_proto::frame::FLAG_MORE];
        rec.extend_from_slice(&chunk);
        if a.tx.send_record(&rec).await.is_err() {
            break;
        }
        sent += chunk.len();
    }
    let end = tokio::time::timeout(Duration::from_secs(5), a.rx.recv())
        .await
        .unwrap();
    assert!(matches!(end, Ok(None) | Err(_)), "{end:?}");
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn burrow_admin_stops_at_once_when_it_cannot_write_the_url_file() {
    let w = world().await;
    let dir = tempfile::tempdir().unwrap();
    let server = if common::ws_mode() {
        format!("ws://{}/burrow", w.addr)
    } else {
        w.addr.to_string()
    };
    let config = dir.path().join("visitor.yaml");
    burrow::instance::write_private(
        &config,
        &format!("server: {server}\nname: ops\nsecret: {}\n", w.ops.encode()),
    )
    .unwrap();
    let missing = dir.path().join("no-such-dir").join("url.txt");
    let started = std::time::Instant::now();
    let run = tokio::task::spawn_blocking(move || {
        std::process::Command::new(env!("CARGO_BIN_EXE_burrow"))
            .arg("admin")
            .arg("-c")
            .arg(&config)
            .args([
                "--no-open",
                "--print-url",
                "--idle-exit",
                "60s",
                "--url-file",
            ])
            .arg(&missing)
            .output()
    });
    let out = tokio::time::timeout(Duration::from_secs(30), run)
        .await
        .expect("burrow admin stopped long before its idle exit")
        .unwrap()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("cannot write"), "{stderr}");
    assert!(!stderr.contains("no browser was connected"), "{stderr}");
    assert!(started.elapsed() < Duration::from_secs(30));
    let _ = w.stop.send(true);
}

struct Local {
    port: u16,
    token: String,
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<Result<(), burrow::failure::Failure>>,
}

async fn local(w: &World, name: &str, secret: &Secret) -> Local {
    let mut s = common::visitor_settings(w.addr, secret.clone(), Vec::new());
    s.name = name.to_string();
    let listener = burrow::admin_local::bind(0).await.unwrap();
    let opts = burrow::admin_local::Options {
        port: 0,
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
    Local {
        port,
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

async fn ws(port: u16, origin: &str) -> Result<Ws, String> {
    let b = tokio_websockets::ClientBuilder::new()
        .uri(&format!("ws://127.0.0.1:{port}/api"))
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
        &format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n", l.port),
    )
    .await;
    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
    assert!(ok.contains("Content-Security-Policy: default-src 'none'"));
    assert!(ok.contains("Cache-Control: no-store"));
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
    let bad_origin = ws(l.port, "http://evil.example").await;
    assert!(bad_origin.is_err());
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
        "GET /api HTTP/1.1\r\nHost: 127.0.0.1:{p}\r\nOrigin: http://127.0.0.1:{p}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
        p = l.port
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
    let mut silent = ws(l.port, &origin).await.unwrap();
    assert!(
        ws_text(&mut silent).await.is_none(),
        "no token, no messages"
    );
    assert!(
        started.elapsed() < Duration::from_secs(7),
        "closed within about 5 s"
    );
    let mut wrong = ws(l.port, &origin).await.unwrap();
    ws_send(&mut wrong, json!({"auth": "0".repeat(64)})).await;
    assert!(ws_text(&mut wrong).await.is_none());
    let mut right = ws(l.port, &origin).await.unwrap();
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
    let e = burrow::admin_local::check_access(&s).await.unwrap_err();
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
    burrow::admin_local::check_access(&s).await.unwrap();
    let l = local(&w, "plain", &w.plain).await;
    let origin = format!("http://127.0.0.1:{}", l.port);
    let mut tab = ws(l.port, &origin).await.unwrap();
    ws_send(&mut tab, json!({"auth": l.token})).await;
    let refused = ws_text(&mut tab).await.unwrap();
    assert_eq!(refused["body"]["state"], "refused");
    assert_eq!(refused["body"]["reason"], "forbidden");
    let _ = l.stop.send(true);
    let _ = w.stop.send(true);
}
