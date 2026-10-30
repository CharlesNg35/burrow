use std::future::Future;
use std::net::ToSocketAddrs;
use std::pin::Pin;
use std::process::Command;
use std::time::Duration;

use super::world;
use super::world::{BIN, Options, World};
use super::{Ctx, SECS, echo_server, gate, open, run};

#[test]
fn local_web_security_holds_against_other_pages_and_processes() {
    if gate().is_none() {
        return;
    }
    let mut w = World::new(Options::default());
    let url = w.open_admin("ops");
    let port: u16 = url
        .trim_start_matches("http://127.0.0.1:")
        .split('/')
        .next()
        .and_then(|p| p.parse().ok())
        .expect("a port in the url");
    let get = |head: String| -> String {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
        s.set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        s.write_all(head.as_bytes()).expect("write");
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        String::from_utf8_lossy(&out).to_string()
    };
    let good = format!("127.0.0.1:{port}");
    let run = url
        .split('#')
        .next()
        .unwrap_or_default()
        .trim_start_matches(&format!("http://{good}"))
        .trim_end_matches('/')
        .to_string();
    assert!(run.starts_with("/r-") && run.len() == 35, "{url}");
    assert!(get("GET / HTTP/1.1\r\nHost: evil.example\r\n\r\n".into()).starts_with("HTTP/1.1 403"));
    assert!(
        get(format!(
            "GET / HTTP/1.1\r\nHost: evil.example:{port}\r\n\r\n"
        ))
        .starts_with("HTTP/1.1 403")
    );
    assert!(
        get(format!(
            "GET / HTTP/1.1\r\nHost: {good}\r\nSec-Fetch-Site: cross-site\r\n\r\n"
        ))
        .starts_with("HTTP/1.1 403")
    );
    let page = get(format!("GET {run}/ HTTP/1.1\r\nHost: {good}\r\n\r\n"));
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    let root = get(format!("GET / HTTP/1.1\r\nHost: {good}\r\n\r\n"));
    assert!(root.starts_with("HTTP/1.1 302"), "{root}");
    assert!(page.contains("Content-Security-Policy: default-src 'none'"));
    assert!(page.contains("script-src 'self' 'wasm-unsafe-eval'"));
    assert!(!page.to_ascii_lowercase().contains("set-cookie"));
    let upgrade_at = |path: &str, origin: &str| {
        format!(
            "GET {path} HTTP/1.1\r\nHost: {good}\r\nOrigin: {origin}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        )
    };
    let upgrade = |origin: &str| upgrade_at(&format!("{run}/api"), origin);
    assert!(
        get(upgrade_at("/api", &format!("http://{good}"))).starts_with("HTTP/1.1 404"),
        "the api answers only under this run's prefix"
    );
    assert!(get(upgrade("http://evil.example")).starts_with("HTTP/1.1 403"));
    let started = std::time::Instant::now();
    let silent = get(upgrade(&format!("http://{good}")));
    assert!(silent.starts_with("HTTP/1.1 101"), "{silent}");
    assert!(
        started.elapsed() < Duration::from_secs(7),
        "a socket without the token is closed in about 5 s"
    );
    w.stop("admin-ops");
}

struct Impostor {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Impostor {
    fn start(port: u16, app_js: String) -> Impostor {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind(("127.0.0.1", port)).expect("the impostor port");
        listener.set_nonblocking(true).expect("nonblocking");
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stop.clone();
        let page = format!(
            "<!doctype html><html><head><script type=\"module\" src=\"{app_js}\"></script><script>document.documentElement.setAttribute('data-pwned', 'page'); const t = sessionStorage.getItem('burrow-admin-token') || (location.hash.match(/token=([0-9a-f]+)/) || [])[1] || ''; const ws = new WebSocket('ws://' + location.host + '/api'); ws.onopen = () => ws.send(JSON.stringify({{auth: t || ''}})); ws.onmessage = () => document.documentElement.setAttribute('data-api', 'ok'); ws.onerror = () => {{ if (!document.documentElement.getAttribute('data-api')) document.documentElement.setAttribute('data-api', 'refused'); }}; ws.onclose = ws.onerror; setTimeout(() => ws.onerror(), 4000);</script></head><body>PWNED impostor</body></html>"
        );
        let trojan = "export default function init() { document.documentElement.setAttribute('data-pwned', 'script'); document.body.textContent = 'PWNED ' + location.hash; }\n".to_string();
        let thread = std::thread::spawn(move || {
            let mut served = Vec::new();
            while !flag.load(std::sync::atomic::Ordering::SeqCst) {
                let Ok((mut s, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(20));
                    continue;
                };
                let (page, trojan) = (page.clone(), trojan.clone());
                served.push(std::thread::spawn(move || {
                    let _ = s.set_nonblocking(false);
                    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
                    let mut buf = [0u8; 4096];
                    let n = s.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    let head = String::from_utf8_lossy(&buf[..n]);
                    let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let (body, kind) = if path.ends_with(".js") {
                        (trojan, "text/javascript")
                    } else if path.contains('.') {
                        (String::new(), "application/octet-stream")
                    } else {
                        (page, "text/html")
                    };
                    let _ = write!(
                        s,
                        "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nCache-Control: public, max-age=31536000, immutable\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                }));
            }
            drop(listener);
            for t in served {
                let _ = t.join();
            }
        });
        Impostor {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Impostor {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn impostor(c: &mut Ctx) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    Box::pin(async move {
        let info = Command::new(world::BIN)
            .args(["admin", "--ui-info"])
            .output()
            .expect("burrow admin --ui-info");
        let app_js = String::from_utf8_lossy(&info.stdout)
            .split_whitespace()
            .find(|w| w.starts_with("/app-") && w.ends_with(".js"))
            .map(str::to_string)
            .unwrap_or_else(|| "/app.js".into());
        let port = world::free_port();
        let fake = Impostor::start(port, app_js.clone());
        c.ui.goto("about:blank").await;
        let _: bool = c.ui.eval("true").await;
        for path in ["/services/web", "/clients", app_js.as_str(), "/"] {
            c.ui.goto(&format!("http://127.0.0.1:{port}{path}")).await;
        }
        c.ui.wait_for("[data-pwned]", SECS).await;
        c.ui.shot("the-impostor-before-burrow-admin").await;
        c.ui.goto("about:blank").await;
        drop(fake);
        let gap = std::time::Instant::now();
        loop {
            let addr = ("127.0.0.1", port)
                .to_socket_addrs()
                .unwrap()
                .next()
                .unwrap();
            if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_err() {
                break;
            }
            assert!(
                gap.elapsed() < Duration::from_secs(10),
                "the impostor still holds port {port}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let url = c.world.open_admin_on("ops", port);
        let path = url
            .trim_start_matches(&format!("http://127.0.0.1:{port}"))
            .split('#')
            .next()
            .unwrap_or("/")
            .to_string();
        let started = std::time::Instant::now();
        loop {
            use std::io::{Read, Write};
            let real = std::net::TcpStream::connect(("127.0.0.1", port))
                .ok()
                .and_then(|mut sock| {
                    write!(
                        sock,
                        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
                    )
                    .ok()?;
                    let mut out = String::new();
                    let _ = sock.read_to_string(&mut out);
                    Some(out)
                })
                .is_some_and(|g| g.contains(&format!("src=\"{path}")));
            if real {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(15),
                "burrow admin did not take over {port}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        let token = url.split("#token=").nth(1).unwrap_or_default().to_string();
        c.ui.goto(&format!("http://127.0.0.1:{port}/#token={token}"))
            .await;
        c.ui.wait_for("[data-api]", SECS).await;
        c.ui.shot("plain-root-from-the-poisoned-cache").await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        let api: String =
            c.ui.eval("document.documentElement.getAttribute('data-api') || ''")
                .await;
        assert_ne!(
            api, "ok",
            "a page from the poisoned cache reached the admin API with the launch token"
        );
        let pwned: bool =
            c.ui.eval("document.documentElement.getAttribute('data-pwned') != null")
                .await;
        let redirected: String = c.ui.eval("location.pathname").await;
        assert!(
            pwned || redirected.starts_with("/r-"),
            "the plain root neither served the cached impostor nor redirected to the run: {redirected}"
        );
        c.ui.take_errors();
        c.ui.take_errors();
    })
}

#[test]
fn an_impostor_that_held_the_port_earlier_never_runs_in_the_real_ui() {
    run("impostor", Options::default(), false, impostor);
}

#[test]
fn a_visitor_without_admin_rights_is_refused_in_the_terminal() {
    let w = World::new(Options {
        clients: Some(Vec::new()),
        ..Options::default()
    });
    let file = w.visitor_file("plain");
    let out = Command::new(BIN)
        .args([
            "admin",
            "-c",
            &file,
            "--no-open",
            "--print-url",
            "--port",
            "0",
        ])
        .current_dir(w.path())
        .env("BURROW_INSTANCE_DIR", w.file("inst-admin"))
        .output()
        .expect("burrow admin");
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(
        err.contains(&format!(
            "visitor \"plain\" has no admin rights on {}",
            w.server_addr()
        )),
        "{err}"
    );
    assert!(err.contains("add \"admin: write\" (or \"admin: read\") to visitors.plain in server.yaml on the server"), "{err}");
    assert!(
        String::from_utf8_lossy(&out.stdout).trim().is_empty(),
        "no link is printed"
    );
}

fn visitor_live(c: &mut Ctx) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    Box::pin(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let echo = echo_server(c.world.ports["echo"]).await;
        open(c, "ops").await;
        c.ui.nav("/visitors/laptop").await;
        c.ui.wait_text("[data-testid=path-diagram]", "not seen since start", SECS)
            .await;
        c.ui.shot("visitor-not-seen-yet").await;
        let file = c.world.visitor_file("laptop");
        let (target, local) = (
            c.world.ports["echo"].to_string(),
            c.world.ports["forward"].to_string(),
        );
        c.world.start(
            "visit",
            &[
                "visit", "-c", &file, "--client", "home", "--target", &target, "--local", &local,
            ],
            &[],
        );
        let port = c.world.ports["forward"];
        let mut held = None;
        for _ in 0..50 {
            if let Ok(s) = tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
                held = Some(s);
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let mut s = held.expect("the visit forward listens");
        s.write_all(b"ping").await.expect("send through the tunnel");
        let mut back = [0u8; 4];
        s.read_exact(&mut back)
            .await
            .expect("echo through the tunnel");
        c.ui.wait_text("[data-testid=path-diagram]", "in use now", SECS)
            .await;
        c.ui.wait_for(
            "[data-testid=entity-list] [data-row=laptop][data-group='In use now']",
            SECS,
        )
        .await;
        c.ui.shot("visitor-in-use-now").await;
        drop(s);
        c.ui.wait_text("[data-testid=path-diagram]", "no open connection", SECS)
            .await;
        c.ui.shot("visitor-last-used").await;
        c.ui.nav("/").await;
        c.ui.wait_text("[data-testid=kpi-visitors]", "2 used since start", SECS)
            .await;
        echo.abort();
        c.ui.assert_no_errors();
    })
}

fn admin_session(c: &mut Ctx) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    Box::pin(async move {
        let port = world::free_port();
        let url = c.world.open_admin_on("ops", port);
        c.ui.goto(&url).await;
        c.ui.wait_text("[data-testid=conn-pill]", "Connected to", SECS)
            .await;
        c.ui.wait_gone("[data-testid=loading]", Duration::from_secs(20))
            .await;
        c.world.stop("admin-ops");
        c.ui.wait_text(
            "[data-testid=banner-reconnecting]",
            "burrow admin is not running",
            Duration::from_secs(20),
        )
        .await;
        c.ui.shot("admin-not-running").await;
        let log = c.world.file("admin-ops.log");
        let _ = std::fs::remove_file(c.world.file("url-ops"));
        let again = c.world.open_admin_on("ops", port);
        assert_eq!(
            again, url,
            "the next run for the same config reuses the session"
        );
        c.ui.wait_text(
            "[data-testid=conn-pill]",
            "Connected to",
            Duration::from_secs(20),
        )
        .await;
        c.ui.wait_gone("[data-testid=banner-reconnecting]", SECS)
            .await;
        world::wait_until("the reconnect note", Duration::from_secs(10), || {
            std::fs::read_to_string(&log)
                .is_ok_and(|t| t.contains("reconnected in your open browser tab"))
        });
        let printed = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(
            !printed.contains("#token="),
            "without --print-url the token is never printed: {printed}"
        );
        c.ui.shot("old-tab-reconnected").await;
        c.world.stop("admin-ops");
        c.ui.wait_text(
            "[data-testid=banner-reconnecting]",
            "burrow admin is not running",
            Duration::from_secs(20),
        )
        .await;
        tokio::time::sleep(Duration::from_secs(45)).await;
        let banner = c.ui.text("[data-testid=banner-reconnecting]").await;
        assert!(
            banner.contains("this tab reconnects") && !banner.contains("0 pending"),
            "45 s after burrow admin stopped the tab still retries every 2 s: {banner}"
        );
        c.ui.shot("admin-gone-for-a-while").await;
        let logged = c.ui.take_errors();
        assert!(
            !logged.iter().any(|l| l.contains("not sent")),
            "no console noise while disconnected: {logged:?}"
        );
        let _ = std::fs::remove_file(c.world.file("url-ops"));
        c.world.open_admin_on("ops", port);
        world::wait_until(
            "the reconnect note after a long outage",
            Duration::from_secs(10),
            || {
                std::fs::read_to_string(&log)
                    .is_ok_and(|t| t.contains("reconnected in your open browser tab"))
            },
        );
        let token = url.split("#token=").nth(1).unwrap().to_string();
        let foreign = url.replace(&token, &"0".repeat(64));
        c.ui.goto("about:blank").await;
        c.ui.goto(&foreign).await;
        c.ui.wait_text(
            "[data-testid=ended]",
            "This tab's admin session ended",
            SECS,
        )
        .await;
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert_eq!(
            c.ui.count("[data-testid=banner-reconnecting]").await,
            0,
            "an ended session does not retry"
        );
        c.ui.shot("admin-session-ended").await;
        let live: bool = c
            .ui
            .eval("!document.querySelector('[data-testid=search-open]').disabled || !document.querySelector('[data-testid=server-switcher]').disabled || !document.querySelector('nav[aria-label=Sections]').inert")
            .await;
        assert!(
            !live,
            "after the session ends the header's controls do nothing and say so"
        );
        c.ui.take_errors();
    })
}

const COUNT_SOCKETS: &str = "(() => { const W = window.WebSocket; window.__sockets = []; const Counted = function (...a) { const s = new W(...a); window.__sockets.push(s); return s; }; Counted.prototype = W.prototype; Object.assign(Counted, W); window.WebSocket = Counted; })()";
const LIVE_SOCKETS: &str = "window.__sockets.filter(s => s.readyState <= 1).length";

fn retry_now(c: &mut Ctx) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    Box::pin(async move {
        let port = world::free_port();
        let url = c.world.open_admin_on("ops", port);
        c.ui.init_script(COUNT_SOCKETS).await;
        c.ui.goto(&url).await;
        c.ui.wait_text("[data-testid=conn-pill]", "Connected to", SECS)
            .await;
        c.world.stop("admin-ops");
        for _ in 0..2 {
            c.ui.wait_for("[data-testid=banner-reconnecting]", Duration::from_secs(20))
                .await;
            let _: bool = c
                .ui
                .eval("([...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Retry now')?.click(), true)")
                .await;
        }
        let _ = std::fs::remove_file(c.world.file("url-ops"));
        c.world.open_admin_on("ops", port);
        c.ui.wait_text(
            "[data-testid=conn-pill]",
            "Connected to",
            Duration::from_secs(20),
        )
        .await;
        c.ui.wait_true(
            &format!("{LIVE_SOCKETS} === 1"),
            "one socket after the retries",
            SECS,
        )
        .await;
        tokio::time::sleep(Duration::from_secs(11)).await;
        let live: u64 = c.ui.eval(LIVE_SOCKETS).await;
        assert_eq!(
            live, 1,
            "no older retry chain opens a second socket within the longest delay"
        );
        let refused = |l: &String| {
            (l.starts_with("log: WebSocket connection to 'ws://")
                && l.ends_with(
                    "failed: Error in connection establishment: net::ERR_CONNECTION_REFUSED ",
                ))
                || (l.starts_with(
                    "log: Failed to load resource: net::ERR_CONNECTION_REFUSED http://",
                ) && l.ends_with("/api"))
        };
        let logged: Vec<String> =
            c.ui.take_errors()
                .into_iter()
                .filter(|l| !refused(l))
                .collect();
        assert!(
            logged.is_empty(),
            "only the refused connections of the outage are logged: {logged:?}"
        );
    })
}

#[test]
fn retry_now_while_reconnecting_keeps_one_socket() {
    run("retry-now", Options::default(), false, retry_now);
}

#[test]
fn an_old_tab_reconnects_to_the_next_burrow_admin_and_a_foreign_one_is_told_it_ended() {
    run("admin-session", Options::default(), false, admin_session);
}

#[test]
fn a_visitor_shows_live_use_while_burrow_visit_carries_a_connection() {
    run("visitor-live", Options::default(), false, visitor_live);
}
