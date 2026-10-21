use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::task::JoinHandle;
use tracing::{debug, warn};

use super::Entry;
use crate::metrics::Status;
use crate::metrics::{Handler, serve_with};

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
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn handler(token: String, snapshot: Arc<dyn Fn() -> Status + Send + Sync>) -> Handler {
    let expected = format!("bearer {token}");
    Arc::new(move |req: &hyper::Request<()>| {
        let authorized = req.headers().get_all("authorization").iter().any(|v| {
            String::from_utf8_lossy(v.as_bytes())
                .trim()
                .eq_ignore_ascii_case(&expected)
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
            ("GET", _) => (404, "text/plain", "not found\n".into()),
            _ => (405, "text/plain", "only GET is supported\n".into()),
        }
    })
}

pub fn publish(
    config: &Path,
    snapshot: Arc<dyn Fn() -> Status + Send + Sync>,
) -> Option<Published> {
    publish_in(&crate::instance::default_dir(), config, snapshot)
}

pub fn publish_in(
    dir: &Path,
    config: &Path,
    snapshot: Arc<dyn Fn() -> Status + Send + Sync>,
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
        config: crate::service::system::canonical_or_same(config),
        pid: std::process::id(),
        port,
        token: token.clone(),
    };
    let path = dir.join(format!("{}.status", entry.pid));
    let text = serde_json::to_string(&entry).expect("entry serializes");
    if let Err(e) =
        crate::instance::prepare(dir).and_then(|()| crate::instance::write_private(&path, &text))
    {
        warn!(
            "burrow status will not see this server: cannot write {} ({e})",
            path.display()
        );
        return None;
    }
    debug!("status for burrow status at {}", path.display());
    let task = tokio::spawn(serve_with(listener, handler(token, snapshot)));
    let fresh = crate::instance::keep_fresh(vec![path.clone()]);
    Some(Published {
        path,
        task,
        _fresh: fresh,
    })
}
