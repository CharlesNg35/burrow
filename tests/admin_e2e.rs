#[path = "admin_e2e/access.rs"]
mod access;
#[path = "admin_e2e/checks.rs"]
mod checks;
#[path = "admin_e2e/dashboard.rs"]
mod dashboard;
#[path = "admin_e2e/edits.rs"]
mod edits;
#[path = "admin_e2e/header.rs"]
mod header;
#[path = "admin_e2e/screens.rs"]
mod screens;
#[path = "admin_e2e/secrets.rs"]
mod secrets;
#[path = "admin_e2e/sheet.rs"]
mod sheet;
#[path = "admin_e2e/ui.rs"]
mod ui;
#[path = "admin_e2e/world.rs"]
mod world;

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Command;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use futures_util::FutureExt;

use checks::{Baseline, ax_problems, compare};
use sheet::Flow;
use ui::Ui;
use world::{BIN, Options, World};

const SECS: Duration = Duration::from_secs(10);

struct Gate {
    chrome: PathBuf,
    chrome_version: String,
    hooks: bool,
}

fn in_ci() -> bool {
    std::env::var("BURROW_E2E_REQUIRE").as_deref() == Ok("1")
}

fn skip(why: &str) -> Option<Gate> {
    assert!(
        !in_ci(),
        "admin e2e is required (BURROW_E2E_REQUIRE=1) but cannot run: {why}"
    );
    println!("skipping the burrow admin end-to-end flow: {why}");
    None
}

fn gate() -> Option<Gate> {
    let info = Command::new(BIN)
        .args(["admin", "--ui-info"])
        .output()
        .expect("burrow admin --ui-info");
    let info = String::from_utf8_lossy(&info.stdout).to_string();
    if info.contains("placeholder") {
        return skip("this burrow has no admin UI built in; run cargo xtask ui first");
    }
    let Some(chrome) = ui::chrome() else {
        panic!(
            "Chrome for Testing {} is missing at {}: run cargo xtask tools to get the pinned Chrome (BURROW_E2E_CHROME or BURROW_E2E_SYSTEM_CHROME=1 use another one)",
            ui::pinned_version(),
            ui::pinned_chrome().display()
        );
    };
    let version = Command::new(&chrome)
        .arg("--version")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let chrome_version = version
        .split_whitespace()
        .find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .unwrap_or("unknown")
        .to_string();
    Some(Gate {
        chrome,
        chrome_version,
        hooks: info.contains("test_hooks: true"),
    })
}

static SLOTS: Mutex<usize> = Mutex::new(0);
static FREED: Condvar = Condvar::new();
const MAX_BROWSERS: usize = 1;

struct Slot;

impl Slot {
    fn take() -> Slot {
        let mut n = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
        while *n >= MAX_BROWSERS {
            n = FREED.wait(n).unwrap_or_else(|e| e.into_inner());
        }
        *n += 1;
        Slot
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut n = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
        *n -= 1;
        FREED.notify_one();
    }
}

pub struct Ctx {
    ui: Ui,
    world: World,
    chrome_version: String,
}

type Body = for<'a> fn(&'a mut Ctx) -> Pin<Box<dyn Future<Output = ()> + 'a>>;

fn run(flow: &str, opts: Options, needs_hooks: bool, body: Body) {
    let Some(g) = gate() else { return };
    if needs_hooks && !g.hooks {
        if in_ci() {
            panic!("{flow} needs a burrow built with --features admin-test-hooks");
        }
        println!("skipping {flow}: build with --features admin-test-hooks");
        return;
    }
    let _slot = Slot::take();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("a runtime");
    let flow_name = flow.to_string();
    rt.block_on(async move {
        let world = World::new(opts);
        let profile = world.file("chrome-profile");
        let ui = Ui::launch(&g.chrome, Flow::new(&flow_name), &profile).await;
        let mut ctx = Ctx {
            ui,
            world,
            chrome_version: g.chrome_version.clone(),
        };
        let result = std::panic::AssertUnwindSafe(body(&mut ctx))
            .catch_unwind()
            .await;
        let errors = ctx.ui.take_errors();
        match result {
            Ok(()) => {
                ctx.ui.flow.finish(true, "");
                ctx.ui.close().await;
            }
            Err(p) => {
                let msg = p
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "panic".into());
                let png = tokio::time::timeout(SECS, ctx.ui.png())
                    .await
                    .unwrap_or_default();
                ctx.ui.flow.failure("after-failure.png", &png);
                ctx.ui
                    .flow
                    .failure("console.txt", errors.join("\n").as_bytes());
                ctx.ui
                    .flow
                    .failure("processes.txt", ctx.world.logs().as_bytes());
                ctx.ui.flow.finish(false, &msg);
                ctx.ui.close().await;
                std::panic::resume_unwind(p);
            }
        }
    });
}

async fn open(c: &mut Ctx, visitor: &str) {
    let url = c.world.open_admin(visitor);
    c.ui.goto(&url).await;
    c.ui.wait_for("[data-testid=conn-pill]", SECS).await;
    c.ui.wait_gone("[data-testid=loading]", Duration::from_secs(20))
        .await;
    let now: String = c.ui.eval("location.href").await;
    assert!(
        !now.contains("token"),
        "the token is still in the address bar: {now}"
    );
}

pub async fn wait_checked(c: &Ctx) {
    c.ui.wait_for("[data-testid=check-state]", SECS).await;
    c.ui.wait_for("[data-testid=check-state]:not([data-state=checking])", SECS)
        .await;
}

pub async fn review_now(c: &Ctx) {
    if c.ui
        .count("[data-testid=save-bar] [data-testid=save]")
        .await
        > 0
        && c.ui
            .enabled("[data-testid=save-bar] [data-testid=save]")
            .await
    {
        c.ui.click("[data-testid=save-bar] [data-testid=save]")
            .await;
        c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
        wait_checked(c).await;
    }
    c.ui.click("[data-testid=review-open]").await;
}

pub async fn edit_mode(c: &Ctx) {
    if c.ui.count("[data-testid=save-bar]").await == 0 {
        c.ui.click("[data-testid=edit]").await;
    }
    c.ui.wait_for("[data-testid=save-bar]", SECS).await;
}

pub async fn open_review(c: &mut Ctx, step: &str) {
    review_now(c).await;
    c.ui.wait_for("[data-testid=review]", SECS).await;
    c.ui.shot(step).await;
}

pub async fn applied(c: &mut Ctx, step: &str) {
    c.ui.wait_for("[data-testid=apply]:not([disabled])", SECS)
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    c.ui.click("[data-testid=apply]").await;
    c.ui.wait_text(
        "[data-testid=toasts]",
        "Applied rev",
        Duration::from_secs(20),
    )
    .await;
    c.ui.wait_gone("[data-testid=review]", SECS).await;
    c.ui.shot(step).await;
}

async fn ax_clean(c: &Ctx, screen: &str) {
    let tree = c.ui.ax_tree().await;
    let problems = ax_problems(&tree);
    assert!(
        problems.is_empty(),
        "accessibility problems on {screen}: {problems:#?}"
    );
}

async fn baseline(c: &mut Ctx, name: &str) {
    let _: bool = c
        .ui
        .eval("(() => { for (const e of document.querySelectorAll('.font-mono, .tabular-nums, [data-testid=server-line], [data-testid=you], [data-tone] .tabular-nums')) e.style.visibility = 'hidden'; return true; })()")
        .await;
    let png = c.ui.png().await;
    let _: bool = c
        .ui
        .eval("(() => { for (const e of document.querySelectorAll('[style]')) e.style.visibility = ''; return true; })()")
        .await;
    match compare(name, &png, &c.chrome_version) {
        Baseline::Same | Baseline::Written => {}
        Baseline::Skipped(why) => println!("baseline {name}: {why}"),
        Baseline::Differs { ratio, diff } => {
            c.ui.flow
                .failure(&format!("baseline-{name}-diff.png"), &diff);
            c.ui.flow
                .failure(&format!("baseline-{name}-actual.png"), &png);
            panic!(
                "screen {name} differs from its baseline in {:.2}% of pixels",
                ratio * 100.0
            );
        }
    }
}

async fn echo_server(port: u16) -> tokio::task::JoinHandle<()> {
    let l = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("echo port");
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    })
}

async fn round_trip(port: u16, bytes: usize) -> std::io::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
    let data = vec![7u8; bytes];
    let (mut r, mut w) = s.split();
    let send = async {
        w.write_all(&data).await?;
        w.shutdown().await
    };
    let mut got = Vec::new();
    let recv = r.read_to_end(&mut got);
    let (a, b) = tokio::join!(send, tokio::time::timeout(SECS, recv));
    a?;
    b.map_err(|_| std::io::Error::other("echo timed out"))??;
    Ok(())
}
