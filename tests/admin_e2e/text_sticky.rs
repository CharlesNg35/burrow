use super::edits::Flow;
use super::text_edit::{EDITOR, MAIN, input, js, start};
use super::world::Options;
use super::{Ctx, SECS, open, run};

const STICKY: &str = ".config-editor__sticky-line";

fn opts() -> Options {
    let fillers: String = (1..=30)
        .map(|i| {
            format!(
                "  app{i:02}:\n    client: home\n    target: {}\n    visitors: [laptop]\n",
                9000 + i
            )
        })
        .collect();
    Options {
        extra_top: "web:\n  http: 127.0.0.1:HTTP_PORT\n".into(),
        extra_services: format!(
            "{fillers}  site:\n    client: home\n    target: 8080\n    web:\n      hosts: [site.example.com]\n      tls: \"off\"\n      request_headers:\n        X-One: \"1\"\n        X-Two: \"2\"\n        X-Three: \"3\"\n        X-Four: \"4\"\n        X-Five: \"5\"\n        X-Six: \"6\"\n      response_headers:\n        X-Seven: \"7\"\n"
        ),
        ..Options::default()
    }
}

async fn scroll_to(c: &Ctx, line: &str) {
    let done: bool = c
        .ui
        .eval(&format!(
            "(() => {{ const t = document.querySelector({}); const row = t.value.split('\\n').findIndex(l => l.trim() === {}); if (row < 0) return false; const s = getComputedStyle(t); t.scrollTop = row * parseFloat(s.lineHeight); t.dispatchEvent(new Event('scroll')); return true; }})()",
            js(&input(MAIN)),
            js(line)
        ))
        .await;
    assert!(done, "server.yaml has the line {line:?}");
}

async fn sticky_lines(c: &Ctx) -> Vec<String> {
    c.ui.eval(&format!(
        "[...document.querySelectorAll('{EDITOR} {STICKY}')].map(l => l.textContent.trim())"
    ))
    .await
}

async fn shows(c: &Ctx, want: &[&str], at: &str) {
    let until = std::time::Instant::now() + SECS;
    loop {
        let now = sticky_lines(c).await;
        if now == want {
            return;
        }
        assert!(
            std::time::Instant::now() < until,
            "at {at}, the sticky lines are {want:?}, not {now:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

async fn row_of(c: &Ctx, line: &str) -> u64 {
    c.ui.eval(&format!(
        "document.querySelector({}).value.split('\\n').findIndex(l => l.trim() === {})",
        js(&input(MAIN)),
        js(line)
    ))
    .await
}

async fn under_the_header(c: &Ctx) {
    let fit: (f64, f64, f64) = c
        .ui
        .eval(&format!(
            "(() => {{ const t = document.querySelector({}).getBoundingClientRect(); const s = [...document.querySelectorAll('{EDITOR} {STICKY}')].map(l => l.getBoundingClientRect()); const h = document.querySelector('[data-testid=header]').getBoundingClientRect(); return [t.top, s.length ? Math.min(...s.map(r => r.top)) : -1, h.bottom]; }})()",
            js(&input(MAIN))
        ))
        .await;
    let (text, first, header) = fit;
    assert!(
        first >= text - 1.0 && first >= header - 1.0,
        "the sticky lines sit at the top of the text, below the page header: text {text}, first sticky line {first}, header {header}"
    );
}

async fn nested_blocks_stick(c: &mut Ctx, width: i64, step: &str) {
    c.ui.viewport(width, 900).await;
    scroll_to(c, "X-Five: \"5\"").await;
    shows(
        c,
        &["services:", "site:", "web:", "request_headers:"],
        "X-Five",
    )
    .await;
    under_the_header(c).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    c.ui.shot(step).await;
    scroll_to(c, "target: 9015").await;
    shows(c, &["services:", "app15:"], "app15").await;
}

async fn a_click_goes_back_to_the_block(c: &Ctx) {
    scroll_to(c, "X-Five: \"5\"").await;
    shows(
        c,
        &["services:", "site:", "web:", "request_headers:"],
        "X-Five",
    )
    .await;
    let row = row_of(c, "site:").await;
    c.ui.click(&format!("{EDITOR} {STICKY}[data-line='{}']", row + 1))
        .await;
    let at_site = format!(
        "(() => {{ const t = document.querySelector({}); const lh = parseFloat(getComputedStyle(t).lineHeight); const top = Math.round(t.scrollTop / lh); const caret = t.value.slice(0, t.selectionStart).split('\\n').length - 1; return top <= {row} && {row} <= top + 6 && caret === {row}; }})()",
        js(&input(MAIN))
    );
    c.ui.wait_true(
        &at_site,
        "a click on a sticky line scrolls back to it and puts the caret on its line",
        SECS,
    )
    .await;
}

fn text_sticky(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        start(c).await;
        nested_blocks_stick(c, 1440, "text-sticky").await;
        nested_blocks_stick(c, 420, "text-sticky-phone").await;
        c.ui.viewport(1440, 900).await;
        a_click_goes_back_to_the_block(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn the_text_editor_keeps_the_blocks_around_the_top_line_in_view() {
    run("text-sticky", opts(), false, text_sticky);
}
