pub mod assets;
pub mod bridge;
pub mod find;
pub mod http;
pub mod launch;
pub mod token;

use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, watch};
use tracing::debug;

use crate::config::settings::VisitorSettings;
use crate::dialer::Dialer;
use crate::failure::Failure;
use http::Route;
use token::Token;

const MAX_OPEN: usize = 32;
const READ_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct Options {
    pub port: Option<u16>,
    pub open: bool,
    pub print_url: bool,
    pub url_file: Option<PathBuf>,
    pub idle_exit: Duration,
    pub ui_dir: Option<PathBuf>,
}

struct Shared {
    port: u16,
    prefix: String,
    token: Token,
    dialer: Arc<Dialer>,
    visitor: String,
    server: String,
    ui_dir: Option<PathBuf>,
    connected: AtomicUsize,
    changed: watch::Sender<u64>,
}

struct Connected(Arc<Shared>);

impl Drop for Connected {
    fn drop(&mut self) {
        self.0.connected.fetch_sub(1, Ordering::AcqRel);
        self.0.changed.send_modify(|n| *n += 1);
    }
}

pub async fn check_access(settings: &VisitorSettings) -> Result<(), Failure> {
    let dialer = Dialer::visitor(settings);
    match dialer.open_admin(&bridge::open_message()).await {
        Ok((_, tx, _)) => {
            tx.close().await;
            Ok(())
        }
        Err(e) => {
            let (reason, message) = bridge::explain(&e, &settings.name, &settings.server);
            let f = Failure::new(message);
            Err(match reason {
                "forbidden" => f.hint(format!(
                    "add \"admin: write\" (or \"admin: read\") to visitors.{} in server.yaml on the server",
                    settings.name
                )),
                "rejected" => f.hint("check name and secret against visitors in the server config"),
                "no-admin" => f.hint("update burrow on the server"),
                _ => f,
            })
        }
    }
}

async fn read_head(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut head = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        if head.len() > http::MAX_HEAD {
            return Err(std::io::Error::other("request header too large"));
        }
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Err(std::io::Error::other("closed before the request ended"));
        }
        head.extend_from_slice(&chunk[..n]);
    }
    Ok(head)
}

async fn reply(stream: &mut TcpStream, code: u16, mime: &str, extra: &str, body: &[u8]) {
    let head = http::response(code, mime, extra, body.len());
    let write = async {
        stream.write_all(head.as_bytes()).await?;
        stream.write_all(body).await?;
        stream.shutdown().await
    };
    let _ = tokio::time::timeout(WRITE_TIMEOUT, write).await;
}

fn from_disk(dir: &std::path::Path, path: &str) -> Option<(Vec<u8>, &'static str)> {
    let name = path.trim_start_matches('/');
    let name = if name.is_empty() { "index.html" } else { name };
    if name.contains('/') || name.contains("..") {
        return None;
    }
    let mime = match name.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css",
        Some("svg") => "image/svg+xml",
        Some("woff2") => "font/woff2",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    };
    std::fs::read(dir.join(name)).ok().map(|b| (b, mime))
}

pub fn index_for_run(index: &[u8], prefix: &str) -> Vec<u8> {
    String::from_utf8_lossy(index)
        .replace("href=\"/", &format!("href=\"{prefix}/"))
        .replace("src=\"/", &format!("src=\"{prefix}/"))
        .into_bytes()
}

const FRESH_INDEX: &str = "Cache-Control: no-store\r\nClear-Site-Data: \"cache\"\r\n";

async fn page(stream: &mut TcpStream, shared: &Shared, path: &str, gzip: bool) {
    let spa = !http::is_file(path);
    if let Some(dir) = shared.ui_dir.as_ref().filter(|_| cfg!(debug_assertions)) {
        let index = spa || path == "/index.html";
        let found = if index {
            from_disk(dir, "/index.html").map(|(b, m)| (index_for_run(&b, &shared.prefix), m))
        } else {
            from_disk(dir, path)
        };
        match found {
            Some((body, mime)) => {
                let extra = if spa {
                    FRESH_INDEX
                } else {
                    "Cache-Control: no-store\r\n"
                };
                reply(stream, 200, mime, extra, &body).await
            }
            None => reply(stream, 404, "text/plain", "", b"not found\n").await,
        }
        return;
    }
    if spa || path == "/index.html" {
        let (body, _) = assets::body(assets::index(), false);
        let body = index_for_run(&body, &shared.prefix);
        reply(stream, 200, assets::index().mime, FRESH_INDEX, &body).await;
        return;
    }
    let Some(asset) = assets::find(path) else {
        reply(stream, 404, "text/plain", "", b"not found\n").await;
        return;
    };
    let (body, encoded) = assets::body(asset, gzip);
    let mut extra = format!("ETag: \"{}\"\r\nCache-Control: no-store\r\n", asset.etag);
    if encoded {
        extra.push_str("Content-Encoding: gzip\r\nVary: Accept-Encoding\r\n");
    }
    reply(stream, 200, asset.mime, &extra, &body).await;
}

async fn connection(mut stream: TcpStream, shared: Arc<Shared>) {
    let head = match tokio::time::timeout(READ_TIMEOUT, read_head(&mut stream)).await {
        Ok(Ok(h)) => h,
        _ => return,
    };
    let parsed = http::parse(&head)
        .and_then(|h| http::check(&h, shared.port, &shared.prefix).map(|r| (h, r)));
    match parsed {
        Err(r) => {
            debug!("burrow admin refused a request: {}", r.reason);
            let body = format!("{}\n", r.reason);
            reply(
                &mut stream,
                r.code,
                "text/plain; charset=utf-8",
                "",
                body.as_bytes(),
            )
            .await;
        }
        Ok((h, Route::Page(path))) => page(&mut stream, &shared, &path, h.gzip).await,
        Ok((_, Route::Moved(to))) => {
            let extra = format!("Location: {to}\r\nCache-Control: no-store\r\n");
            reply(&mut stream, 302, "text/plain; charset=utf-8", &extra, b"").await;
        }
        Ok((_, Route::Api)) => {
            shared.connected.fetch_add(1, Ordering::AcqRel);
            shared.changed.send_modify(|n| *n += 1);
            let _guard = Connected(shared.clone());
            bridge::serve(
                stream,
                head,
                shared.token.clone(),
                shared.dialer.clone(),
                shared.visitor.clone(),
                shared.server.clone(),
            )
            .await;
        }
    }
}

pub struct Running {
    pub addr: SocketAddr,
    pub url: String,
}

pub const DEFAULT_ADMIN_PORT: u16 = 7839;

pub async fn listen(
    port: Option<u16>,
    default: u16,
) -> Result<(TcpListener, Option<String>), Failure> {
    if let Some(p) = port {
        return Ok((bind(p).await?, None));
    }
    let why = match TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], default))).await {
        Ok(l) => return Ok((l, None)),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            format!("port {default} is in use")
        }
        Err(e) => format!("cannot use port {default} ({e})"),
    };
    let l = bind(0).await?;
    let note = l
        .local_addr()
        .ok()
        .map(|a| format!("{why}; the UI is on 127.0.0.1:{}", a.port()));
    Ok((l, note))
}

pub async fn bind(port: u16) -> Result<TcpListener, Failure> {
    TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port)))
        .await
        .map_err(|e| {
            Failure::new(format!("cannot listen on 127.0.0.1:{port}: {e}"))
                .hint("pick another --port, or 0 for a free one")
        })
}

pub async fn serve(
    listener: TcpListener,
    settings: &VisitorSettings,
    opts: &Options,
    mut stop: watch::Receiver<bool>,
    ready: impl FnOnce(&Running) -> Result<(), Failure>,
) -> Result<(), Failure> {
    let addr = listener
        .local_addr()
        .map_err(|e| Failure::new(format!("cannot read the local address: {e}")))?;
    let token = Token::generate();
    let prefix = http::run_prefix();
    let url = format!(
        "http://127.0.0.1:{}{prefix}/#token={}",
        addr.port(),
        token.as_str()
    );
    let (changed, mut watch_changes) = watch::channel(0u64);
    let shared = Arc::new(Shared {
        port: addr.port(),
        prefix,
        token,
        dialer: Arc::new(Dialer::visitor(settings)),
        visitor: settings.name.clone(),
        server: settings.server.clone(),
        ui_dir: opts.ui_dir.clone(),
        connected: AtomicUsize::new(0),
        changed,
    });
    ready(&Running { addr, url })?;
    let open = Arc::new(Semaphore::new(MAX_OPEN));
    let mut idle_since = tokio::time::Instant::now();
    loop {
        let idle_left = opts.idle_exit.saturating_sub(idle_since.elapsed());
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                };
                let Ok(permit) = open.clone().try_acquire_owned() else {
                    continue;
                };
                let shared = shared.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    connection(stream, shared).await;
                });
            }
            _ = watch_changes.changed() => {
                if shared.connected.load(Ordering::Acquire) == 0 {
                    idle_since = tokio::time::Instant::now();
                }
            }
            _ = tokio::time::sleep(idle_left), if shared.connected.load(Ordering::Acquire) == 0 => {
                crate::say_errln!(
                    "no browser was connected for {}; burrow admin stopped (run it again, or pass a longer --idle-exit)",
                    crate::client::reconnect::human(opts.idle_exit)
                );
                return Ok(());
            }
            _ = stop.changed() => return Ok(()),
        }
    }
}

pub fn announce(
    out: &mut dyn Write,
    settings: &VisitorSettings,
    running: &Running,
    opts: &Options,
    opened: bool,
) {
    let mut lines = vec![format!(
        "burrow admin for {} (visitor \"{}\") listens on http://{}",
        settings.server, settings.name, running.addr
    )];
    if opts.print_url {
        lines.push(format!("open: {}", running.url));
    } else if opened {
        lines.push("opened in your browser; run with --print-url to see the link".into());
    } else {
        lines.push("run with --print-url to see the link to open".into());
    }
    lines.push("stop it with Ctrl-C".into());
    for l in lines {
        let _ = writeln!(out, "{l}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_default_port_falls_back_with_the_real_reason() {
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let busy = held.local_addr().unwrap().port();
        let (l, note) = listen(None, busy).await.unwrap();
        assert_ne!(l.local_addr().unwrap().port(), busy);
        let note = note.unwrap();
        assert!(
            note.starts_with(&format!("port {busy} is in use; the UI is on 127.0.0.1:")),
            "{note}"
        );
        if std::net::TcpListener::bind("127.0.0.1:1").is_err() {
            let (_, note) = listen(None, 1).await.unwrap();
            let note = note.unwrap();
            assert!(note.starts_with("cannot use port 1 ("), "{note}");
        }
        let (l, note) = listen(Some(0), busy).await.unwrap();
        assert!(note.is_none());
        assert_ne!(l.local_addr().unwrap().port(), busy);
        assert!(listen(Some(busy), 0).await.is_err());
    }
}
