mod common;
#[path = "admin/grants.rs"]
mod grants;
#[path = "admin/launch.rs"]
mod launch;
#[path = "admin/local_page.rs"]
mod local_page;
#[path = "admin/session.rs"]
mod session;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use burrow::config::{Loaded, Role, load_file};
use burrow::dialer::Dialer;
use burrow::dialer::admin::{AdminOpenError, AdminRx, AdminTx};
use burrow::server::Server;
use burrow::wire::secret::Secret;
use burrow_admin_proto::frame::AdminOpen;
use burrow_admin_proto::msg::{ErrorBody, Incoming};
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

use common::free_port;

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
    world_checked(burrow::app::reload::accept()).await
}

async fn world_checked(
    check: burrow::app::reload::Check<burrow::config::settings::ServerSettings>,
) -> World {
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
    burrow::base::fs::write_private(&path, &text).unwrap();
    let env = burrow::config::env::Env::default();
    let outcome = load_file(&path, Some(Role::Server), &env).unwrap();
    let report = outcome
        .report
        .render_text(burrow::base::style::Style { color: false });
    let Loaded::Server(settings, _) = outcome.loaded.unwrap_or_else(|| panic!("{report}")) else {
        panic!("server config")
    };
    let server = Server::bind(*settings.clone()).await.unwrap();
    let addr = server.local_addr().or(server.websocket_addr()).unwrap();
    let shared = server.shared();
    let (hub, nudges) = burrow::admin::adapters::hub(&shared, &path, env.clone());
    shared.set_admin(hub);
    let (reload_tx, reload_rx) = mpsc::channel(4);
    tokio::spawn(burrow::app::reload::follow_nudged(
        path.clone(),
        *settings,
        env,
        check,
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
