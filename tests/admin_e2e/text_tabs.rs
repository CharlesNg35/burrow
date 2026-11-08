use std::time::Duration;

use super::edits::Flow;
use super::text_edit::{
    CARD, CHECKED, EDITOR, GOTO, MAIN, PANEL, close_of, dirty, go_to, input, js, key, replace_in,
    show, start, tab, text_of,
};
use super::world::Options;
use super::{Ctx, SECS, open, run};

const FILES: usize = 12;
const MAX_TABS: usize = 10;
const REVIEW: &str = "[data-testid=file-review]";
const MORE: &str = "[data-testid=file-goto-more]";

fn name(i: usize) -> String {
    format!("server.d/web-{i}.yaml")
}

fn opts() -> Options {
    Options {
        drop_ins: (1..=FILES)
            .map(|i| {
                let file: &'static str = Box::leak(format!("web-{i}.yaml").into_boxed_str());
                (
                    file,
                    format!(
                        "services:\n  svc{i}:\n    client: home\n    target: {}\n    visitors: [laptop]\n",
                        3000 + i
                    ),
                    false,
                )
            })
            .collect(),
        ..Options::default()
    }
}

async fn tabs(c: &Ctx) -> Vec<String> {
    c.ui.all_attrs(&format!("{EDITOR} [role=tab]"), "data-document-id")
        .await
}

async fn hits(c: &Ctx) -> Vec<String> {
    c.ui.eval("[...document.querySelectorAll('[data-testid=file-goto-hit] button > span:nth-child(2)')].map(s => s.textContent)")
        .await
}

async fn file_hits(c: &Ctx) -> usize {
    hits(c)
        .await
        .iter()
        .filter(|h| h.starts_with("server.d/"))
        .count()
}

async fn heads(c: &Ctx) -> Vec<String> {
    c.ui.eval("[...document.querySelectorAll('#file-goto-list > li[role=presentation]')].map(s => s.textContent)")
        .await
}

async fn typed(c: &Ctx, query: &str) {
    if c.ui.count(PANEL).await == 0 {
        c.ui.click(GOTO).await;
    }
    c.ui.fill("#file-goto-input", query).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
}

async fn press(c: &Ctx, key: &str) {
    let _: bool =
        c.ui.eval("(() => { document.querySelector('#file-goto-input').focus(); return true; })()")
            .await;
    c.ui.press("#file-goto-input", key).await;
}

async fn focused(c: &Ctx, id: &str, what: &str) {
    c.ui.wait_true(
        &format!(
            "!!document.activeElement && document.activeElement.id === {}",
            js(id)
        ),
        what,
        SECS,
    )
    .await;
}

async fn on_screen(c: &Ctx, css: &str) -> bool {
    c.ui.eval(&format!(
        "(() => {{ const e = document.querySelector({}); if (!e) return false; const r = e.getBoundingClientRect(); return r.width > 0 && r.left >= 0 && r.right <= innerWidth + 0.5; }})()",
        js(css)
    ))
    .await
}

async fn both_widths(c: &mut Ctx, step: &str) {
    for w in [1440, 390] {
        c.ui.viewport(w, 900).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let wide: bool = c
            .ui
            .eval("document.documentElement.scrollWidth > document.documentElement.clientWidth")
            .await;
        assert!(!wide, "{step} at {w}px scrolls sideways");
        if c.ui.count(PANEL).await > 0 {
            assert!(on_screen(c, PANEL).await, "{step}: Go to fits at {w}px");
        }
        c.ui.shot(&format!("{step}-{w}")).await;
    }
    c.ui.viewport(1440, 900).await;
}

async fn go_to_finds_and_opens(c: &mut Ctx) {
    start(c).await;
    assert_eq!(tabs(c).await, [MAIN], "Edit opens the main file only");
    key(c, MAIN, "p", "KeyP", "ctrlKey: true,").await;
    c.ui.wait_for(PANEL, SECS).await;
    focused(
        c,
        "file-goto-input",
        "Ctrl+P in the editor puts the caret in Go to",
    )
    .await;
    assert_eq!(c.ui.attr_of("#file-goto-input", "role").await, "combobox");
    assert_eq!(c.ui.attr_of("#file-goto-list", "role").await, "listbox");
    assert_eq!(
        hits(c).await,
        [MAIN],
        "an empty query offers recent files and the main file"
    );
    both_widths(c, "text-tabs-goto-empty").await;
    press(c, "Escape").await;
    c.ui.wait_gone(PANEL, SECS).await;
    focused(
        c,
        "file-goto",
        "Esc gives the focus back to the Go to button",
    )
    .await;
    typed(c, "web-1").await;
    let found = hits(c).await;
    assert_eq!(found.len(), 4, "{found:?}");
    assert_eq!(found[0], name(1), "the exact name comes first: {found:?}");
    typed(c, "w12").await;
    assert_eq!(hits(c).await, [name(12)], "letters in order find a file");
    typed(c, "web").await;
    assert_eq!(file_hits(c).await, 8, "the first eight per group");
    c.ui.wait_text(MORE, "4 more", SECS).await;
    both_widths(c, "text-tabs-goto-files").await;
    c.ui.click(&format!("{MORE} button")).await;
    c.ui.wait_true(
        "[...document.querySelectorAll('[data-testid=file-goto-hit] button > span:nth-child(2)')].filter(s => s.textContent.startsWith('server.d/')).length === 12",
        "… 4 more shows the rest",
        SECS,
    )
    .await;
    typed(c, "web-1").await;
    press(c, "ArrowDown").await;
    press(c, "Enter").await;
    c.ui.wait_gone(PANEL, SECS).await;
    c.ui.wait_for(&format!("{}[aria-selected=true]", tab(&name(10))), SECS)
        .await;
    assert_eq!(tabs(c).await, [MAIN.to_string(), name(10)]);
    go_to(c, "svc7", "svc7").await;
    c.ui.wait_for(&format!("{}[aria-selected=true]", tab(&name(7))), SECS)
        .await;
    c.ui.wait_true(
        &format!(
            "(() => {{ const i = document.querySelector({}); return !!i && document.activeElement === i && i.selectionStart === i.value.indexOf('svc7'); }})()",
            js(&input(&name(7)))
        ),
        "a service opens its file at the service",
        SECS,
    )
    .await;
    go_to(c, "target: 3005", "target: 3005").await;
    c.ui.wait_for(&format!("{}[aria-selected=true]", tab(&name(5))), SECS)
        .await;
    c.ui.wait_true(
        &format!(
            "(() => {{ const i = document.querySelector({}); return !!i && document.activeElement === i && i.selectionStart === i.value.indexOf('target: 3005'); }})()",
            js(&input(&name(5)))
        ),
        "a text match opens its file at its line",
        SECS,
    )
    .await;
}

async fn ten_tabs_at_most(c: &mut Ctx) {
    replace_in(c, &name(10), "target: 3010", "target: 4010").await;
    dirty(c, &name(10), true).await;
    for i in [1, 2, 3, 4, 6, 8, 9, 11, 12] {
        go_to(c, &name(i), &name(i)).await;
        c.ui.wait_for(&format!("{}[aria-selected=true]", tab(&name(i))), SECS)
            .await;
        assert!(tabs(c).await.len() <= MAX_TABS);
    }
    let want: Vec<String> = std::iter::once(MAIN.to_string())
        .chain([10, 2, 3, 4, 6, 8, 9, 11, 12].map(name))
        .collect();
    assert_eq!(
        tabs(c).await,
        want,
        "the least recently used clean tabs close; the main file and the edited one stay"
    );
    both_widths(c, "text-tabs-ten").await;
    go_to(c, "web-7", &name(7)).await;
    c.ui.wait_for(&input(&name(7)), SECS).await;
    assert!(text_of(c, &name(7)).await.contains("target: 3007"));
    assert!(
        !tabs(c).await.contains(&name(2)),
        "reopening closes the next oldest"
    );
}

async fn closing_tabs(c: &mut Ctx) {
    c.ui.click(&close_of(&name(12))).await;
    c.ui.wait_gone(&tab(&name(12)), SECS).await;
    assert_eq!(
        c.ui.count("[data-testid=file-close-dialog]").await,
        0,
        "a tab without edits closes at once"
    );
    assert_eq!(
        c.ui.count(&close_of(MAIN)).await,
        0,
        "the main file stays open"
    );
    c.ui.click(&close_of(&name(10))).await;
    c.ui.wait_for("[data-testid=file-close-dialog]", SECS).await;
    c.ui.shot("text-tabs-close-asks").await;
    c.ui.click("[data-testid=file-close-keep]").await;
    c.ui.wait_gone("[data-testid=file-close-dialog]", SECS)
        .await;
    dirty(c, &name(10), true).await;
    c.ui.click(&close_of(&name(10))).await;
    c.ui.click("[data-testid=file-close-confirm]").await;
    c.ui.wait_gone(&tab(&name(10)), SECS).await;
    show(c, &name(10)).await;
    assert!(
        text_of(c, &name(10)).await.contains("target: 3010"),
        "a discarded tab comes back as the server has it"
    );
    dirty(c, &name(10), false).await;
    assert!(!c.ui.enabled(REVIEW).await, "nothing left to review");
}

async fn problems_in_closed_files_are_counted(c: &mut Ctx) {
    show(c, MAIN).await;
    replace_in(c, MAIN, "\n  home:\n", "\n  house:\n").await;
    let badge = "[data-testid=file-goto-problems]";
    c.ui.wait_for(badge, CHECKED).await;
    let n: usize = c.ui.text(badge).await.trim().parse().expect("a count");
    assert!(n > 0, "files that are not open and have problems: {n}");
    let label = c.ui.attr_of(GOTO, "aria-label").await;
    assert!(
        label.contains("have problems") || label.contains("has problems"),
        "{label}"
    );
    c.ui.click(GOTO).await;
    c.ui.wait_for(PANEL, SECS).await;
    let groups = heads(c).await;
    assert!(
        groups.iter().any(|h| h == "Files with problems"),
        "{groups:?}"
    );
    both_widths(c, "text-tabs-problems").await;
    press(c, "Escape").await;
    c.ui.viewport(390, 900).await;
    assert!(on_screen(c, GOTO).await, "Go to is on screen at 390px");
    let words: bool = c
        .ui
        .eval("getComputedStyle(document.querySelector('[data-testid=file-goto] span.truncate')).display === 'none'")
        .await;
    assert!(words, "on a phone Go to is an icon");
    c.ui.viewport(1440, 900).await;
    c.ui.click("[data-testid=file-cancel]").await;
    c.ui.click("[data-testid=file-discard]").await;
    c.ui.wait_for(&format!("{CARD}[data-editing=false]"), SECS)
        .await;
}

fn text_tabs(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        go_to_finds_and_opens(c).await;
        ten_tabs_at_most(c).await;
        closing_tabs(c).await;
        problems_in_closed_files_are_counted(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn edit_opens_files_on_demand_with_go_to_and_keeps_at_most_ten_tabs() {
    run("text-tabs", opts(), false, text_tabs);
}
