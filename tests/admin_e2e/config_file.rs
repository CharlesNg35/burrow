use std::time::Duration;

use super::edits::Flow;
use super::world::Options;
use super::{Ctx, SECS, adornments_clear, open, run};

const TABS: &str = "[data-testid=settings-tabs]";
const TEXT: &str = "[data-testid=file-text] pre";
const NOTE: &str = "[data-testid=file-differs]";
const ENV_VALUE: &str = "e2e-env-file-token-6d1c";
const HEADER: &str = "Bearer e2e-header-credential-7f2a";
const CAPTURE: &str = "(() => { window.__copied = null; window.__saved = null; window.__name = null; navigator.clipboard.writeText = t => { window.__copied = t; return Promise.resolve(); }; URL.createObjectURL = b => { b.text().then(t => { window.__saved = t; }); return 'blob:captured'; }; URL.revokeObjectURL = () => {}; HTMLAnchorElement.prototype.click = function () { window.__name = this.download; }; return true; })()";

fn long_services(n: usize) -> String {
    (0..n)
        .map(|i| {
            format!(
                "  app{i:02}:\n    client: home\n    target: {}\n    visitors: [laptop]\n    allow_from: [10.{i}.0.0/16, 192.168.{i}.0/24, 2001:db8:{i:x}::/48]\n",
                9000 + i
            )
        })
        .collect()
}

fn site(hash: &str) -> String {
    format!(
        "  site:\n    client: home\n    target: 8080\n    web:\n      hosts: [site.example.com, a-rather-long-host-name-that-keeps-going.internal.example.com]\n      tls: \"off\"\n      basic_auth:\n        alice: \"{hash}\"\n      request_headers: {{ Authorization: \"{HEADER}\", X-Env-Token: \"${{UP_TOKEN}}\" }}\n"
    )
}

fn web_long() -> Options {
    let hash = burrow::server::passwords::hash(b"e2e password").expect("a hash");
    Options {
        extra_top: "web:\n  http: 127.0.0.1:HTTP_PORT\n".into(),
        extra_services: format!("{}{}", site(&hash), long_services(40)),
        server_env: vec![("UP_TOKEN".into(), ENV_VALUE.into())],
        ..Options::default()
    }
}

async fn file_tab(c: &Ctx) -> String {
    c.ui.nav("/settings?tab=file").await;
    c.ui.wait_for(&format!("{TABS} [data-tab=file][aria-selected=true]"), SECS)
        .await;
    assert_eq!(
        c.ui.all_attrs(&format!("{TABS} [role=tab]"), "data-tab")
            .await
            .last()
            .map(String::as_str),
        Some("file"),
        "Raw config is the last Settings tab"
    );
    let name: String =
        c.ui.eval(&format!(
            "document.querySelector('{TABS} [data-tab=file]').textContent.trim()"
        ))
        .await;
    assert!(
        name.starts_with("Raw config"),
        "the tab is named Raw config: {name:?}"
    );
    c.ui.wait_text(TEXT, "secrets show as fingerprints", SECS)
        .await
}

fn secret_free(c: &Ctx, text: &str, hash: Option<&str>) {
    for (name, s) in &c.world.secrets {
        assert!(!text.contains(s.as_str()), "the secret of {name} shows");
    }
    for v in [ENV_VALUE, HEADER].into_iter().chain(hash) {
        assert!(!text.contains(v), "{v} shows");
    }
    assert!(!text.contains("$argon2"), "a password hash shows");
}

async fn defaults(c: &Ctx, on: bool) -> String {
    let (want, gone) = if on {
        ("heartbeat:", "")
    } else {
        ("services:", "heartbeat:")
    };
    c.ui.click("[data-testid=file-toggle] label").await;
    c.ui.wait_true(
        &format!(
            "(() => {{ const t = document.querySelector('{TEXT}')?.textContent ?? ''; return t.includes('{want}') && ('{gone}' === '' || !t.includes('{gone}')); }})()"
        ),
        "the view follows Show defaults too",
        SECS,
    )
    .await;
    c.ui.text(TEXT).await
}

async fn copy_and_download(c: &Ctx) -> (String, String, String) {
    let _: bool = c.ui.eval(CAPTURE).await;
    c.ui.click("[data-testid=file-text] button:not([aria-label])")
        .await;
    c.ui.click("[data-testid=file-text] button[aria-label^=Download]")
        .await;
    c.ui.wait_true(
        "window.__copied !== null && window.__saved !== null && window.__name !== null",
        "the copy and the download",
        SECS,
    )
    .await;
    (
        c.ui.eval("window.__copied").await,
        c.ui.eval("window.__saved").await,
        c.ui.eval("window.__name").await,
    )
}

async fn shots(c: &mut Ctx, step: &str) {
    for (w, size) in [(1440, "1440"), (1280, "1280"), (390, "390")] {
        c.ui.viewport(w, 900).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let wide: bool = c
            .ui
            .eval("document.documentElement.scrollWidth > document.documentElement.clientWidth")
            .await;
        assert!(!wide, "{step} at {w}px scrolls sideways");
        let inside: bool = c
            .ui
            .eval("(() => { const p = document.querySelector('[data-testid=file-text]').getBoundingClientRect(); const card = document.querySelector('[data-testid=config-file]').getBoundingClientRect(); return p.right <= card.right + 0.5 && p.left >= card.left - 0.5; })()")
            .await;
        assert!(inside, "{step} at {w}px: the text stays inside its card");
        adornments_clear(c, step).await;
        c.ui.shot(&format!("{step}-{size}")).await;
    }
    c.ui.viewport(1440, 900).await;
}

const BOX: &str = "(() => { const p = document.querySelector('[data-testid=file-text] pre'); return { inner: p.scrollHeight, shown: p.clientHeight, page: document.documentElement.scrollHeight, window: innerHeight, role: p.getAttribute('role'), tab: p.tabIndex, name: p.getAttribute('aria-label'), sideways: p.scrollWidth > p.clientWidth }; })()";

async fn a_long_file_scrolls_in_its_own_box(c: &mut Ctx) {
    for (w, h, size) in [
        (1440, 900, "1440"),
        (390, 844, "390"),
        (844, 390, "landscape"),
    ] {
        c.ui.viewport(w, h).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let b: serde_json::Value = c.ui.eval(BOX).await;
        let n = |k: &str| b[k].as_f64().unwrap_or(0.0);
        assert!(
            n("inner") > n("shown") + 10.0,
            "{size}: the text scrolls inside its box: {b}"
        );
        assert!(
            n("shown") <= n("window") * 0.8 + 1.0,
            "{size}: at most 80% of the window: {b}"
        );
        assert!(
            n("shown") >= 240.0_f64.min(n("window")) - 1.0,
            "{size}: never uselessly small: {b}"
        );
        assert!(
            n("page") < n("inner"),
            "{size}: the page does not grow with the text: {b}"
        );
        assert_eq!(b["role"], "region", "{size}: a named region: {b}");
        assert_eq!(b["tab"], 0, "{size}: reachable by keyboard: {b}");
        assert_eq!(
            b["name"], "server.redacted.yaml",
            "{size}: named after the file: {b}"
        );
        c.ui.view_shot(&format!("file-long-scroll-{size}")).await;
    }
    c.ui.viewport(1440, 900).await;
}

async fn differs_exactly_when_the_file_does(c: &mut Ctx) {
    assert_eq!(c.ui.count(NOTE).await, 0, "the server runs its file");
    c.world
        .hand_edit(|t| t.replace("target: 8080", "target: nowhere:0"));
    c.ui.wait_text(NOTE, "differs from this file", Duration::from_secs(20))
        .await;
    c.ui.wait_true(
        "(() => { const e = document.querySelector('[data-testid=config-file]'); return e && e.dataset.rev !== '' && e.dataset.rev === e.dataset.stateRev; })()",
        "the shown text follows the file's new version",
        SECS,
    )
    .await;
    c.ui.wait_text(TEXT, "nowhere:0", SECS).await;
    shots(c, "file-differs").await;
    c.ui.click("[data-testid=file-differs-link]").await;
    c.ui.wait_text(
        "[data-testid=attention]",
        "The config file has errors",
        SECS,
    )
    .await;
    c.world
        .hand_edit(|t| t.replace("target: nowhere:0", "target: 8080"));
    file_tab(c).await;
    c.ui.wait_true(
        &format!("!document.querySelector('{NOTE}')"),
        "the note goes once the server runs the file again",
        Duration::from_secs(20),
    )
    .await;
}

fn web_and_long(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        let hash = c.world.config();
        let hash = hash
            .split('"')
            .find(|p| p.starts_with("$argon2"))
            .map(str::to_string);
        let text = file_tab(c).await;
        secret_free(c, &text, hash.as_deref());
        assert!(text.contains("«password hash»") && text.contains("${UP_TOKEN}"));
        assert!(text.contains("«secret ") && text.contains("app39:"));
        assert!(!text.contains("heartbeat:"), "only what is written");
        assert_eq!(
            c.ui.count("[data-testid=settings-search]:not(.hidden)")
                .await,
            0
        );
        assert_eq!(
            c.ui.count("[data-testid=edit]").await,
            0,
            "no form to edit here"
        );
        assert_eq!(
            c.ui.count("[data-testid=file-edit]").await,
            1,
            "a write admin may edit the text"
        );
        shots(c, "file-written").await;
        a_long_file_scrolls_in_its_own_box(c).await;
        let all = defaults(c, true).await;
        secret_free(c, &all, hash.as_deref());
        assert!(all.contains("with every default in effect") && all.contains("\nweb:\n"));
        let (copied, saved, name) = copy_and_download(c).await;
        assert_eq!(
            copied.trim_end(),
            all.trim_end(),
            "the copy is the shown text"
        );
        assert_eq!(saved, copied, "the download is the copied text");
        assert_eq!(name, "server.redacted.yaml");
        secret_free(c, &saved, hash.as_deref());
        shots(c, "file-defaults").await;
        defaults(c, false).await;
        differs_exactly_when_the_file_does(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn the_config_file_tab_shows_a_long_web_config_without_secrets() {
    run("config-file-web", web_long(), false, web_and_long);
}

fn read_only(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "viewer").await;
        let text = file_tab(c).await;
        secret_free(c, &text, None);
        assert!(
            text.contains("viewer:") && !text.contains("\nweb:"),
            "{text}"
        );
        let all = defaults(c, true).await;
        assert!(!all.contains("\nweb:"), "the web proxy stays off: {all}");
        assert_eq!(
            c.ui.count("[data-testid=config-file] input:not([type=checkbox])")
                .await,
            0
        );
        assert_eq!(c.ui.count("[data-testid=config-file] textarea").await, 0);
        assert_eq!(
            c.ui.count("[data-testid=file-edit]").await,
            0,
            "a read admin cannot edit the text"
        );
        assert_eq!(c.ui.count(NOTE).await, 0);
        shots(c, "file-read-only").await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_read_admin_sees_the_config_file_and_no_web_proxy_when_it_is_off() {
    run("config-file-read", Options::default(), false, read_only);
}
