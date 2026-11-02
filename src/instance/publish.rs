use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::metrics::Status;
use crate::metrics::{Handler, serve_with};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub config: PathBuf,
    pub pid: u32,
    pub port: u16,
    pub token: String,
}

pub struct Published {
    path: PathBuf,
    task: JoinHandle<()>,
    _fresh: crate::instance::Fresh,
}

impl Published {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Published {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_file(&self.path);
    }
}

fn token() -> String {
    let mut bytes = [0u8; 24];
    getrandom::fill(&mut bytes).expect("operating system random source is available");
    hex::encode(bytes)
}

pub const UPDATE_PATH: &str = "/clients/update";

pub type Snapshot = Arc<dyn Fn() -> Status + Send + Sync>;
pub type Update = Arc<dyn Fn(&[u8]) -> (u16, String) + Send + Sync>;

fn handler(token: String, snapshot: Snapshot, update: Option<Update>) -> Handler {
    Arc::new(move |req: &hyper::Request<()>, body: &[u8]| {
        use subtle::ConstantTimeEq;
        let authorized = req.headers().get_all("authorization").iter().any(|v| {
            let v = v.as_bytes().trim_ascii();
            v.get(..7)
                .is_some_and(|scheme| scheme.eq_ignore_ascii_case(b"bearer "))
                && bool::from(v[7..].ct_eq(token.as_bytes()))
        });
        if !authorized {
            return (401, "text/plain", "a token is required\n".into());
        }
        let target = req.uri().path_and_query().map_or("", |p| p.as_str());
        match (req.method().as_str(), target) {
            ("GET", "/status") => (
                200,
                "application/json",
                serde_json::to_string(&snapshot()).expect("status serializes"),
            ),
            ("POST", UPDATE_PATH) if update.is_some() => {
                let (code, text) = update.as_ref().map_or((404, String::new()), |u| u(body));
                (code, "application/json", text)
            }
            (_, UPDATE_PATH) if update.is_some() => {
                (405, "text/plain", "only POST is supported here\n".into())
            }
            (_, "/status") => (405, "text/plain", "only GET is supported here\n".into()),
            _ => (404, "text/plain", "not found\n".into()),
        }
    })
}

pub fn publish(config: &Path, snapshot: Snapshot, update: Option<Update>) -> Option<Published> {
    publish_in(&crate::instance::default_dir(), config, snapshot, update)
}

pub fn publish_in(
    dir: &Path,
    config: &Path,
    snapshot: Snapshot,
    update: Option<Update>,
) -> Option<Published> {
    let listener = match std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.set_nonblocking(true).map(|()| l))
        .and_then(tokio::net::TcpListener::from_std)
    {
        Ok(l) => l,
        Err(e) => {
            warn!("burrow status will not see this server: cannot open its status port ({e})");
            return None;
        }
    };
    let port = listener.local_addr().ok()?.port();
    let token = token();
    let entry = Entry {
        config: crate::base::fs::canonical_or_same(config),
        pid: std::process::id(),
        port,
        token: token.clone(),
    };
    let path = dir.join(format!("{}.{}", entry.pid, super::STATUS_EXTENSION));
    let text = serde_json::to_string(&entry).expect("entry serializes");
    if let Err(e) =
        crate::instance::prepare(dir).and_then(|()| crate::base::fs::write_private(&path, &text))
    {
        warn!(
            "burrow status will not see this server: cannot write {} ({e})",
            path.display()
        );
        return None;
    }
    debug!("status for burrow status at {}", path.display());
    let task = tokio::spawn(serve_with(listener, handler(token, snapshot, update)));
    let fresh = crate::instance::keep_fresh(vec![path.clone()]);
    Some(Published {
        path,
        task,
        _fresh: fresh,
    })
}
