use std::net::SocketAddr;
use std::path::PathBuf;

use burrow::config::{Loaded, Role, load_file};
use burrow::server::Server;
use burrow::wire::secret::Secret;
use tokio::sync::{mpsc, watch};

use super::free_port;

pub struct World {
    pub _dir: tempfile::TempDir,
    pub path: PathBuf,
    pub addr: SocketAddr,
    pub home: Secret,
    pub ops: Secret,
    pub viewer: Secret,
    pub plain: Secret,
    pub stop: watch::Sender<bool>,
}

fn listener_line() -> String {
    if super::ws_mode() {
        format!(
            "websocket:\n  listen: 127.0.0.1:{}\n  path: /burrow\n",
            free_port()
        )
    } else {
        format!("listen: 127.0.0.1:{} # tcp\n", free_port())
    }
}

pub async fn world() -> World {
    world_checked(burrow::app::reload::accept()).await
}

pub async fn world_checked(
    check: burrow::app::reload::Check<burrow::config::settings::ServerSettings>,
) -> World {
    super::more_files();
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
