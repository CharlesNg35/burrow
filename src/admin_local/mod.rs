pub mod assets;
pub mod bridge;
pub mod find;
pub mod http;
pub mod launch;
pub mod session;
pub mod token;

use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, watch};
use tracing::debug;

use crate::config::settings::VisitorSettings;
use crate::dialer::Dialer;
use crate::failure::Failure;
use http::Decision;
use token::Token;

pub const MAX_OPEN: usize = 32;
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

impl Options {
    pub fn shows_link(&self) -> bool {
        self.print_url || self.open || self.url_file.is_some()
    }
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

fn plain(code: u16, text: &str) -> Response<http::Body> {
    http::response(
        code,
        "text/plain; charset=utf-8",
        &[],
        format!("{text}\n").into_bytes(),
    )
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

fn index_for_run(index: &[u8], prefix: &str) -> Vec<u8> {
    String::from_utf8_lossy(index)
        .replace("href=\"/", &format!("href=\"{prefix}/"))
        .replace("src=\"/", &format!("src=\"{prefix}/"))
        .into_bytes()
}

fn fresh_index() -> Vec<(&'static str, String)> {
    vec![
        ("Cache-Control", "no-store".into()),
        ("Clear-Site-Data", "\"cache\"".into()),
    ]
}

fn page(shared: &Shared, path: &str, spa: bool, gzip: bool) -> Response<http::Body> {
    if let Some(dir) = shared.ui_dir.as_ref().filter(|_| cfg!(debug_assertions)) {
        let index = spa || path == "/index.html";
        let found = if index {
            from_disk(dir, "/index.html").map(|(b, m)| (index_for_run(&b, &shared.prefix), m))
        } else {
            from_disk(dir, path)
        };
        return match found {
            Some((body, mime)) => {
                let extra = if spa {
                    fresh_index()
                } else {
                    vec![("Cache-Control", "no-store".into())]
                };
                http::response(200, mime, &extra, body)
            }
            None => http::response(404, "text/plain", &[], &b"not found\n"[..]),
        };
    }
    if spa || path == "/index.html" {
        let (body, _) = assets::body(assets::index(), false);
        let body = index_for_run(&body, &shared.prefix);
        return http::response(200, assets::index().mime, &fresh_index(), body);
    }
    let Some(asset) = assets::find(path) else {
        return http::response(404, "text/plain", &[], &b"not found\n"[..]);
    };
    let (body, encoded) = assets::body(asset, gzip);
    let mut extra = vec![
        ("ETag", format!("\"{}\"", asset.etag)),
        ("Cache-Control", "no-store".into()),
    ];
    if encoded {
        extra.push(("Content-Encoding", "gzip".into()));
        extra.push(("Vary", "Accept-Encoding".into()));
    }
    http::response(200, asset.mime, &extra, body.into_owned())
}

fn accept_key<B>(req: &Request<B>) -> Option<String> {
    use base64::Engine;
    let h = req.headers();
    let has = |name: &str, want: &str, whole: bool| {
        h.get(name).and_then(|v| v.to_str().ok()).is_some_and(|v| {
            if whole {
                v.eq_ignore_ascii_case(want)
            } else {
                v.to_ascii_lowercase().contains(&want.to_ascii_lowercase())
            }
        })
    };
    if !has("upgrade", crate::transport::ws::UPGRADE_TOKEN, true)
        || !has("connection", "upgrade", false)
        || !has("sec-websocket-version", "13", true)
    {
        return None;
    }
    let key = h.get("sec-websocket-key")?.as_bytes();
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY);
    ctx.update(key);
    ctx.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    Some(base64::engine::general_purpose::STANDARD.encode(ctx.finish()))
}

type Bridge = Arc<std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>;

fn handle(req: Request<Incoming>, shared: &Arc<Shared>, bridge: &Bridge) -> Response<http::Body> {
    let decided = http::head_of(&req)
        .and_then(|h| http::check(&h, shared.port, &shared.prefix).map(|d| (h, d)));
    match decided {
        Err(r) => {
            debug!("burrow admin refused a request: {}", r.reason);
            plain(r.code, r.reason)
        }
        Ok((h, Decision::Page(path))) => page(shared, &path, true, h.gzip),
        Ok((h, Decision::Asset(path))) => page(shared, &path, false, h.gzip),
        Ok((_, Decision::Redirect(to))) => http::response(
            302,
            "text/plain; charset=utf-8",
            &[("Location", to), ("Cache-Control", "no-store".into())],
            Vec::new(),
        ),
        Ok((_, Decision::NotFound | Decision::Reject)) => plain(404, "not found"),
        Ok((_, Decision::Api)) => {
            let Some(accept) = accept_key(&req) else {
                return plain(400, "bad request");
            };
            let shared = shared.clone();
            let task = tokio::spawn(async move {
                let Ok(upgraded) = hyper::upgrade::on(req).await else {
                    return;
                };
                shared.connected.fetch_add(1, Ordering::AcqRel);
                shared.changed.send_modify(|n| *n += 1);
                let _guard = Connected(shared.clone());
                bridge::serve(
                    TokioIo::new(upgraded),
                    shared.token.clone(),
                    shared.dialer.clone(),
                    shared.visitor.clone(),
                    shared.server.clone(),
                )
                .await;
            });
            if let Ok(mut slot) = bridge.lock() {
                *slot = Some(task);
            }
            Response::builder()
                .status(101)
                .header("Upgrade", crate::transport::ws::UPGRADE_TOKEN)
                .header("Connection", "Upgrade")
                .header("Sec-WebSocket-Accept", accept)
                .body(http::Body::default())
                .unwrap_or_else(|_| plain(500, "error"))
        }
    }
}

async fn connection(stream: TcpStream, shared: Arc<Shared>) {
    let bridge: Bridge = Arc::new(std::sync::Mutex::new(None));
    let service = {
        let (shared, bridge) = (shared.clone(), bridge.clone());
        service_fn(move |req| {
            let response = handle(req, &shared, &bridge);
            async move { Ok::<_, std::convert::Infallible>(response) }
        })
    };
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(READ_TIMEOUT)
        .max_buf_size(http::MAX_HEAD)
        .max_header_size(http::MAX_HEAD)
        .keep_alive(false)
        .title_case_headers(true)
        .auto_date_header(false);
    let served = builder
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades();
    let _ = tokio::time::timeout(READ_TIMEOUT + WRITE_TIMEOUT, served).await;
    let task = bridge.lock().ok().and_then(|mut b| b.take());
    if let Some(task) = task {
        let _ = task.await;
    }
}

pub struct Running {
    pub addr: SocketAddr,
    pub url: String,
    pub reused: bool,
}

pub async fn listen(
    port: Option<u16>,
    default: u16,
) -> Result<(TcpListener, Option<String>), Failure> {
    listen_near(port, None, default, false).await
}

pub async fn listen_near(
    port: Option<u16>,
    last: Option<u16>,
    default: u16,
    link: bool,
) -> Result<(TcpListener, Option<String>), Failure> {
    if let Some(p) = port {
        return Ok((bind(p).await?, None));
    }
    let local = |p: u16| SocketAddr::from(([127, 0, 0, 1], p));
    let mut moved = None;
    if let Some(p) = last.filter(|p| *p != default) {
        match TcpListener::bind(local(p)).await {
            Ok(l) => return Ok((l, None)),
            Err(e) => moved = Some((p, e)),
        }
    }
    let (l, failed) = match TcpListener::bind(local(default)).await {
        Ok(l) => (l, None),
        Err(e) => (bind(0).await?, Some((default, e))),
    };
    let note = match (moved.or(failed), l.local_addr()) {
        (Some((p, e)), Ok(a)) => {
            let kept = last == Some(p);
            let whose = if kept { " of your last session" } else { "" };
            let why = match e.kind() {
                std::io::ErrorKind::AddrInUse if kept => format!("port {p}{whose} is taken"),
                std::io::ErrorKind::AddrInUse => format!("port {p} is in use"),
                _ => format!("cannot use port {p}{whose} ({e})"),
            };
            let then = match (kept, link) {
                (false, _) => "",
                (true, true) => " (open the new link)",
                (true, false) => " (run with --print-url to see the new link)",
            };
            Some(format!("{why}; the UI is on 127.0.0.1:{}{then}", a.port()))
        }
        _ => None,
    };
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
    stop: watch::Receiver<bool>,
    ready: impl FnOnce(&Running) -> Result<(), Failure>,
) -> Result<(), Failure> {
    serve_session(
        listener,
        settings,
        opts,
        session::Session::fresh(),
        None,
        stop,
        ready,
        |_, _| {},
    )
    .await
}

pub const RECONNECT_GRACE: Duration = Duration::from_secs(3);
const TOUCH_EVERY: Duration = Duration::from_secs(60);

#[allow(clippy::too_many_arguments)]
pub async fn serve_session(
    listener: TcpListener,
    settings: &VisitorSettings,
    opts: &Options,
    session: session::Session,
    keep: Option<session::Keep>,
    mut stop: watch::Receiver<bool>,
    ready: impl FnOnce(&Running) -> Result<(), Failure>,
    then: impl FnOnce(&Running, bool),
) -> Result<(), Failure> {
    let addr = listener
        .local_addr()
        .map_err(|e| Failure::new(format!("cannot read the local address: {e}")))?;
    let reused = session.reused && session.port == Some(addr.port());
    let touch = |s: &session::Session| -> Option<tokio::task::JoinHandle<()>> {
        let k = keep.as_ref()?;
        let (dir, config, s, port) = (k.dir.clone(), k.config.clone(), s.clone(), addr.port());
        Some(tokio::task::spawn_blocking(move || {
            if let Err(e) = session::save(&dir, &config, &s, port, session::now()) {
                debug!("cannot keep the admin session in {}: {e}", dir.display());
            }
        }))
    };
    let last_touch = |s: &session::Session| {
        let pending = touch(s);
        async move {
            if let Some(p) = pending {
                let _ = p.await;
            }
        }
    };
    touch(&session);
    let url = format!(
        "http://127.0.0.1:{}{}/#token={}",
        addr.port(),
        session.prefix,
        session.token.as_str()
    );
    let (changed, mut watch_changes) = watch::channel(0u64);
    let shared = Arc::new(Shared {
        port: addr.port(),
        prefix: session.prefix.clone(),
        token: session.token.clone(),
        dialer: Arc::new(Dialer::visitor(settings)),
        visitor: settings.name.clone(),
        server: settings.server.clone(),
        ui_dir: opts.ui_dir.clone(),
        connected: AtomicUsize::new(0),
        changed,
    });
    let running = Running { addr, url, reused };
    ready(&running)?;
    let mut then = Some(then);
    let grace = tokio::time::Instant::now() + RECONNECT_GRACE;
    if !reused && let Some(f) = then.take() {
        f(&running, false);
    }
    let mut touched = tokio::time::Instant::now();
    let mut refresh = tokio::time::interval_at(
        tokio::time::Instant::now() + session::REFRESH_EVERY,
        session::REFRESH_EVERY,
    );
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
                } else {
                    if let Some(f) = then.take() {
                        f(&running, true);
                    }
                    if touched.elapsed() >= TOUCH_EVERY {
                        touched = tokio::time::Instant::now();
                        touch(&session);
                    }
                }
            }
            _ = refresh.tick() => {
                if shared.connected.load(Ordering::Acquire) > 0 {
                    touched = tokio::time::Instant::now();
                    touch(&session);
                }
            }
            _ = tokio::time::sleep_until(grace), if then.is_some() => {
                if let Some(f) = then.take() {
                    f(&running, false);
                }
            }
            _ = tokio::time::sleep(idle_left), if shared.connected.load(Ordering::Acquire) == 0 => {
                crate::say_errln!(
                    "no browser was connected for {}; burrow admin stopped (run it again, or pass a longer --idle-exit)",
                    crate::client::reconnect::human(opts.idle_exit)
                );
                last_touch(&session).await;
                return Ok(());
            }
            _ = stop.changed() => {
                last_touch(&session).await;
                return Ok(());
            }
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

    #[test]
    fn a_url_file_counts_as_showing_the_link() {
        let opts = |print_url, open, url_file: Option<&str>| Options {
            port: None,
            open,
            print_url,
            url_file: url_file.map(PathBuf::from),
            idle_exit: Duration::from_secs(1),
            ui_dir: None,
        };
        assert!(!opts(false, false, None).shows_link());
        assert!(opts(true, false, None).shows_link());
        assert!(opts(false, true, None).shows_link());
        assert!(opts(false, false, Some("url.txt")).shows_link());
    }

    #[test]
    fn the_websocket_accept_key_follows_rfc_6455() {
        let req = |key: Option<&str>, version: &str| {
            let mut b = Request::builder()
                .uri("/api")
                .header("Upgrade", "websocket")
                .header("Connection", "keep-alive, Upgrade")
                .header("Sec-WebSocket-Version", version);
            if let Some(k) = key {
                b = b.header("Sec-WebSocket-Key", k);
            }
            b.body(()).unwrap()
        };
        assert_eq!(
            accept_key(&req(Some("dGhlIHNhbXBsZSBub25jZQ=="), "13")).as_deref(),
            Some("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")
        );
        assert_eq!(accept_key(&req(None, "13")), None);
        assert_eq!(
            accept_key(&req(Some("dGhlIHNhbXBsZSBub25jZQ=="), "8")),
            None
        );
    }

    fn free_port() -> u16 {
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        free.local_addr().unwrap().port()
    }

    #[tokio::test]
    async fn a_taken_session_port_is_named_when_the_ui_moves() {
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let last = held.local_addr().unwrap().port();
        let (l, note) = listen_near(None, Some(last), free_port(), true)
            .await
            .unwrap();
        let got = l.local_addr().unwrap().port();
        assert_ne!(got, last);
        assert_eq!(
            note.as_deref(),
            Some(
                format!(
                    "port {last} of your last session is taken; the UI is on 127.0.0.1:{got} (open the new link)"
                )
                .as_str()
            )
        );
        let (l, note) = listen_near(None, Some(last), free_port(), false)
            .await
            .unwrap();
        let got = l.local_addr().unwrap().port();
        assert_eq!(
            note.as_deref(),
            Some(
                format!(
                    "port {last} of your last session is taken; the UI is on 127.0.0.1:{got} (run with --print-url to see the new link)"
                )
                .as_str()
            )
        );
    }

    #[tokio::test]
    async fn a_taken_session_port_on_the_default_is_named_like_any_other() {
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let busy = held.local_addr().unwrap().port();
        let (l, note) = listen_near(None, Some(busy), busy, true).await.unwrap();
        let got = l.local_addr().unwrap().port();
        assert_ne!(got, busy);
        assert_eq!(
            note.as_deref(),
            Some(
                format!(
                    "port {busy} of your last session is taken; the UI is on 127.0.0.1:{got} (open the new link)"
                )
                .as_str()
            )
        );
    }

    #[tokio::test]
    async fn a_session_port_that_cannot_be_used_gives_the_real_reason() {
        if std::net::TcpListener::bind("127.0.0.1:1").is_ok() {
            return;
        }
        let (l, note) = listen_near(None, Some(1), free_port(), true).await.unwrap();
        let got = l.local_addr().unwrap().port();
        let note = note.unwrap();
        assert!(
            note.starts_with("cannot use port 1 of your last session (")
                && note.ends_with(&format!(
                    "); the UI is on 127.0.0.1:{got} (open the new link)"
                )),
            "{note}"
        );
        assert!(!note.contains("taken"), "{note}");
    }

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
