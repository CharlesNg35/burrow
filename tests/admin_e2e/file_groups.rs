use std::time::Duration;

use super::edits::Flow;
use super::world::{self, Options};
use super::{Ctx, SECS, open, run};

const HEADS: &str = "[data-testid=entity-list] [data-testid=file-group]";

fn opts() -> Options {
    Options {
        drop_ins: vec![
            (
                "apps.yaml",
                format!(
                    "services:\n  blog:\n    client: home\n    bind: 127.0.0.1:{}\n    target: 22\n",
                    world::free_port()
                ),
                false,
            ),
            (
                "tool.yaml",
                format!(
                    "services:\n  wiki:\n    client: home\n    target: 23\n    visitors: [laptop]\nclients:\n  box:\n    secret: {}\n",
                    world::secret()
                ),
                true,
            ),
            (
                "zz-down.yaml",
                "services:\n  dead:\n    client: box\n    target: 25\n    visitors: [laptop]\n"
                    .into(),
                false,
            ),
        ],
        ..Options::default()
    }
}

fn head(file: &str) -> String {
    format!("{HEADS}[data-file='{file}']")
}

async fn heads(c: &Ctx) -> Vec<String> {
    c.ui.all_attrs(HEADS, "data-file").await
}

async fn shots(c: &mut Ctx, step: &str) {
    tokio::time::sleep(Duration::from_millis(300)).await;
    c.ui.shot(step).await;
    c.ui.viewport(390, 844).await;
    c.ui.nav("/services").await;
    c.ui.wait_for(HEADS, SECS).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let wide: bool =
        c.ui.eval("document.documentElement.scrollWidth > document.documentElement.clientWidth")
            .await;
    assert!(!wide, "{step} at 390 px scrolls sideways");
    c.ui.shot(&format!("{step}-phone")).await;
    c.ui.viewport(1440, 900).await;
}

async fn press_enter(c: &Ctx, css: &str) {
    let focused: bool = c
        .ui
        .eval(&format!(
            "(() => {{ const b = document.querySelector(\"{css}\"); b.focus(); return document.activeElement === b; }})()"
        ))
        .await;
    assert!(focused, "the file header takes the keyboard focus");
    c.ui.press(css, "Enter").await;
}

async fn search(c: &Ctx, text: &str) {
    c.ui.fill("#list-search", text).await;
    tokio::time::sleep(Duration::from_millis(250)).await;
}

async fn sections_follow_the_files(c: &mut Ctx) {
    c.ui.nav("/services").await;
    c.ui.wait_for(HEADS, SECS).await;
    let mode: String =
        c.ui.eval("document.querySelector('[data-testid=list-group] select').value")
            .await;
    assert_eq!(mode, "file", "four files group by file by default");
    assert_eq!(
        heads(c).await,
        [
            "server.yaml",
            "server.d/apps.yaml",
            "server.d/tool.yaml",
            "server.d/zz-down.yaml"
        ],
        "server.yaml first, then the drop-ins in the order burrow reads them"
    );
    let tip =
        c.ui.attr_of(
            &format!("{} [data-testid=group-lock]", head("server.d/tool.yaml")),
            "title",
        )
        .await;
    assert!(tip.starts_with("Read-only on disk:"), "{tip}");
    assert_eq!(
        c.ui.count("[data-testid=group-lock]").await,
        1,
        "only the read-only file has a lock"
    );
    let down = head("server.d/zz-down.yaml");
    c.ui.click(&down).await;
    c.ui.wait_for(&format!("{down}[aria-expanded=false]"), SECS)
        .await;
    c.ui.wait_text(
        &format!("{down} [data-testid=group-problems]"),
        "1 problem",
        Duration::from_secs(20),
    )
    .await;
    assert_eq!(
        c.ui.count("[data-group-file='server.d/zz-down.yaml'] [data-testid=list-row]")
            .await,
        0,
        "a closed file renders only its header"
    );
    press_enter(c, &down).await;
    c.ui.wait_for(&format!("{down}[aria-expanded=true]"), SECS)
        .await;
    assert_eq!(
        c.ui.count(&format!("{down} [data-testid=group-problems]"))
            .await,
        0,
        "an open file shows its items' problems on the items"
    );
    press_enter(c, &down).await;
    c.ui.wait_for(&format!("{down}[aria-expanded=false]"), SECS)
        .await;
    shots(c, "group-by-file").await;
}

async fn search_opens_and_hides(c: &mut Ctx) {
    c.ui.nav("/services").await;
    c.ui.wait_for(HEADS, SECS).await;
    search(c, "dead").await;
    c.ui.wait_true(
        &format!("document.querySelectorAll(\"{HEADS}\").length === 1"),
        "only the file with a match shows",
        SECS,
    )
    .await;
    let down = head("server.d/zz-down.yaml");
    assert_eq!(c.ui.attr_of(&down, "aria-expanded").await, "true");
    c.ui.wait_for(
        "[data-group-file='server.d/zz-down.yaml'] [data-testid=list-row]",
        SECS,
    )
    .await;
    search(c, "").await;
    c.ui.wait_true(
        &format!("document.querySelectorAll(\"{HEADS}\").length === 4"),
        "every file is back",
        SECS,
    )
    .await;
    assert_eq!(
        c.ui.attr_of(&down, "aria-expanded").await,
        "false",
        "clearing the search brings back the folded file"
    );
    c.ui.click("[data-testid=entity-list] [data-filter=visitors]")
        .await;
    c.ui.wait_true(
        &format!("document.querySelectorAll(\"{HEADS}\").length === 3"),
        "the Visitors chip drops apps.yaml, whose blog has no visitors",
        SECS,
    )
    .await;
    assert_eq!(
        heads(c).await,
        ["server.yaml", "server.d/tool.yaml", "server.d/zz-down.yaml"]
    );
    c.ui.click("[data-testid=entity-list] [data-filter=all]")
        .await;
}

async fn none_and_memory(c: &mut Ctx) {
    c.ui.nav("/services").await;
    c.ui.wait_for(HEADS, SECS).await;
    c.ui.select("[data-testid=list-group] select", "none").await;
    c.ui.wait_gone(HEADS, SECS).await;
    assert!(
        c.ui.count("[data-testid=entity-list] [data-testid=list-row]")
            .await
            >= 4,
        "the flat list shows every service"
    );
    c.ui.nav("/clients").await;
    c.ui.wait_for(HEADS, SECS).await;
    assert_eq!(
        heads(c).await,
        ["server.yaml", "server.d/tool.yaml"],
        "Clients group by their own files"
    );
    c.ui.nav("/services").await;
    c.ui.wait_for("[data-testid=entity-list] [data-testid=list-row]", SECS)
        .await;
    assert_eq!(c.ui.count(HEADS).await, 0, "None is kept for Services");
    c.ui.select("[data-testid=list-group] select", "file").await;
    c.ui.wait_for(HEADS, SECS).await;
    assert_eq!(
        c.ui.attr_of(&head("server.d/zz-down.yaml"), "aria-expanded")
            .await,
        "false",
        "the folded file stays folded for the session"
    );
}

fn groups(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        sections_follow_the_files(c).await;
        search_opens_and_hides(c).await;
        none_and_memory(c).await;
        c.ui.assert_no_errors();
    })
}

fn without_drop_ins(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/services").await;
        c.ui.wait_for("[data-testid=entity-list] [data-testid=list-row]", SECS)
            .await;
        assert_eq!(c.ui.count("[data-testid=list-group]").await, 0);
        assert_eq!(c.ui.count(HEADS).await, 0);
        c.ui.assert_no_errors();
    })
}

fn many() -> Options {
    let drop_ins = (0..100)
        .map(|f| {
            let name: &'static str = Box::leak(format!("f{f:03}.yaml").into_boxed_str());
            let text: String = std::iter::once("services:\n".to_string())
                .chain((0..10).map(|i| format!("  s{f:03}x{i}:\n    client: home\n    target: {}\n    visitors: [laptop]\n", 1000 + f * 10 + i)))
                .collect();
            (name, text, false)
        })
        .collect();
    Options {
        drop_ins,
        ..Options::default()
    }
}

const TIMED: &str = "(async () => { const t = performance.now(); ACT; await new Promise(r => requestAnimationFrame(() => setTimeout(r, 0))); return performance.now() - t; })()";

async fn timed(c: &Ctx, act: &str) -> f64 {
    c.ui.eval(&TIMED.replace("ACT", act)).await
}

fn large(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/services").await;
        c.ui.wait_for(HEADS, Duration::from_secs(30)).await;
        assert_eq!(c.ui.count(HEADS).await, 101);
        let search = "const i = document.querySelector('#list-search'); i.value = 'TEXT'; i.dispatchEvent(new Event('input', { bubbles: true }))";
        let mut typed = Vec::new();
        for text in ["s", "s0", "s05", "s050", "s050x", "s050x3", ""] {
            typed.push(timed(c, &search.replace("TEXT", text)).await);
        }
        let fold = format!(
            "document.querySelector(\"{}\").click()",
            head("server.d/f050.yaml")
        );
        let folding = [timed(c, &fold).await, timed(c, &fold).await];
        let all =
            "document.querySelectorAll(\"HEADS[aria-expanded=true]\").forEach(b => b.click())"
                .replace("HEADS", HEADS);
        let fold_all = timed(c, &all).await;
        let rows_left =
            c.ui.count("[data-testid=entity-list] [data-testid=list-row]")
                .await;
        let typed_folded = timed(c, &search.replace("TEXT", "s07")).await;
        timed(c, &search.replace("TEXT", "")).await;
        c.ui.select("[data-testid=list-group] select", "none").await;
        c.ui.wait_gone(HEADS, SECS).await;
        let flat: Vec<f64> = vec![
            timed(c, &search.replace("TEXT", "s")).await,
            timed(c, &search.replace("TEXT", "s0")).await,
            timed(c, &search.replace("TEXT", "")).await,
        ];
        let line = format!(
            "1000 services in 101 files: typing {typed:?} ms; fold/open one {folding:?} ms; fold all {fold_all:.0} ms ({rows_left} rows left); typing with all folded {typed_folded:.0} ms; flat list typing {flat:?} ms\n"
        );
        let _ = std::fs::write(
            super::sheet::shots_dir().join("file-groups-large/timings.txt"),
            line,
        );
        assert!(
            rows_left <= 2,
            "folded files render only their headers; server.yaml stays open around the selected service"
        );
        assert!(
            typed.iter().chain(&folding).all(|ms| *ms < 1500.0),
            "the list stays responsive: {typed:?} {folding:?}"
        );
    })
}

#[test]
fn a_thousand_items_in_a_hundred_files_stay_responsive() {
    run("file-groups-large", many(), false, large);
}

#[test]
fn lists_group_their_items_by_config_file() {
    run("file-groups", opts(), false, groups);
}

#[test]
fn without_drop_ins_the_list_has_no_grouping() {
    run(
        "file-groups-none",
        Options::default(),
        false,
        without_drop_ins,
    );
}
