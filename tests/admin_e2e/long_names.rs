use std::time::Duration;

use super::edits::Flow;
use super::text_edit::{go_to, js, start};
use super::world::Options;
use super::{Ctx, SECS, open, run};

fn stem(end: &str) -> String {
    let mut s = "a-drop-in-file-name-that-keeps-going-".repeat(4);
    s.truncate(100 - end.len());
    format!("{s}{end}")
}

fn editable() -> String {
    format!("server.d/{}.yaml", stem("-edit"))
}

fn opts() -> Options {
    let lock: &'static str = Box::leak(format!("{}.yaml", stem("-lock")).into_boxed_str());
    let edit: &'static str = Box::leak(format!("{}.yaml", stem("-edit")).into_boxed_str());
    Options {
        drop_ins: vec![
            (
                lock,
                "services:\n  longsvc:\n    client: home\n    target: 7777\n    visitors: [laptop]\n"
                    .into(),
                true,
            ),
            (
                edit,
                "services:\n  longedit:\n    client: home\n    target: 7778\n    visitors: [laptop]\n"
                    .into(),
                false,
            ),
        ],
        ..Options::default()
    }
}

const FITS: &str = "(() => { const w = innerWidth; const out = []; if (document.scrollingElement.scrollWidth > w + 1) out.push('page ' + document.scrollingElement.scrollWidth + ' > ' + w); for (const e of document.querySelectorAll('[data-testid=file-tag], [data-testid=row-file], [data-testid=group-file], [data-testid=file-group], [data-testid=config-file] .font-mono, [data-testid=file-diff] h4')) { const r = e.getBoundingClientRect(); const p = e.parentElement.getBoundingClientRect(); if (r.width > 0 && (r.right > p.right + 1 || r.right > w + 1)) out.push((e.dataset.testid || e.tagName) + ' ' + Math.round(r.right) + ' > ' + Math.round(Math.min(p.right, w))); } return out; })()";

/// For each icon+text pair: the gap between the icon's ink centre (its drawn
/// shapes, scaled from the viewBox) and the middle of the text's cap height
/// (the baseline, found with a zero-size inline-block placed before the text, so on its first line,
/// minus half the height of "H"), in CSS pixels, and the icon's ink
/// height over that cap height.
const INK: &str = "(() => { const c = document.createElement('canvas').getContext('2d'); const out = []; for (const t of document.querySelectorAll('span[class*=\"h-[18px]\"], [data-testid=read-only-file], [data-testid=file-group], button, [role=tab]')) { const svg = t.querySelector(':scope > svg, :scope > span > svg'); if (!svg || svg.getBoundingClientRect().width === 0) continue; const walker = document.createTreeWalker(t, NodeFilter.SHOW_TEXT); let node = null, n; while ((n = walker.nextNode())) { if (n.textContent.trim() && !n.parentElement.closest('.sr-only') && n.parentElement.getBoundingClientRect().width > 0) { node = n; break; } } if (!node) continue; const cs = getComputedStyle(node.parentElement); c.font = cs.fontStyle + ' ' + cs.fontWeight + ' ' + cs.fontSize + ' ' + cs.fontFamily; const m = c.measureText('H'); const probe = document.createElement('span'); probe.style.cssText = 'display:inline-block;width:0;height:0;vertical-align:baseline'; node.before(probe); const base = probe.getBoundingClientRect().bottom; probe.remove(); const text = base - m.actualBoundingBoxAscent / 2; const s = svg.getBoundingClientRect(); const vb = svg.viewBox.baseVal; const b = svg.getBBox(); const icon = s.top + (b.y - vb.y + b.height / 2) * s.height / (vb.height || 24); const ink = b.height * s.height / (vb.height || 24); out.push({ tag: (t.dataset.testid || t.textContent.trim().slice(0, 24)), off: Math.round((icon - text) * 10) / 10, size: Math.round(ink / m.actualBoundingBoxAscent * 100) / 100 }); } return out; })()";

async fn fits(c: &Ctx, step: &str) {
    let wide: Vec<String> = c.ui.eval(FITS).await;
    assert!(
        wide.is_empty(),
        "{step}: wider than its container: {wide:?}"
    );
}

async fn centred(c: &Ctx, step: &str) {
    let found: Vec<serde_json::Value> = c.ui.eval(INK).await;
    eprintln!("ink offsets, {step}: {found:?}");
    let off: Vec<&serde_json::Value> = found
        .iter()
        .filter(|f| {
            f["off"].as_f64().unwrap_or(9.0).abs() > 1.0 || f["size"].as_f64().unwrap_or(9.0) > 1.4
        })
        .collect();
    assert!(
        !found.is_empty() && off.is_empty(),
        "{step}: an icon sits more than 1 px off its text's middle or stands over 1.4 times its cap height: {off:?} (all: {found:?})"
    );
}

async fn titled(c: &Ctx, css: &str, full: &str, step: &str) {
    let ok: bool = c
        .ui
        .eval(&format!(
            "[...document.querySelectorAll({})].some(e => (e.title || '').includes({}) || !!e.querySelector('[title*=' + JSON.stringify({}) + ']'))",
            js(css),
            js(full),
            js(full)
        ))
        .await;
    assert!(ok, "{step}: {css} holds the full name {full} in a title");
}

async fn both_widths(c: &mut Ctx, step: &str, check: &[(&str, &str)]) {
    for w in [1440, 390] {
        c.ui.viewport(w, 900).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        fits(c, &format!("{step} at {w}px")).await;
        for (css, full) in check {
            titled(c, css, full, &format!("{step} at {w}px")).await;
        }
        c.ui.view_shot(&format!("{step}-{w}")).await;
    }
    c.ui.viewport(1440, 900).await;
}

async fn the_detail_page_keeps_its_actions(c: &mut Ctx) {
    c.ui.nav("/services/longsvc").await;
    c.ui.wait_for("[data-testid=file-tag]", SECS).await;
    c.ui.wait_for("[data-testid=read-only-file]", SECS).await;
    centred(c, "the read-only file tag on the detail page").await;
    for w in [1440, 390] {
        c.ui.viewport(w, 900).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let edit: bool = c
            .ui
            .eval("(() => { const b = document.querySelector('[data-testid=edit]'); if (!b) return true; const r = b.getBoundingClientRect(); return r.width > 0 && r.left >= 0 && r.right <= innerWidth; })()")
            .await;
        assert!(edit, "the header actions stay on screen at {w}px");
    }
    both_widths(
        c,
        "long-name-detail",
        &[("[data-testid=file-tag]", &stem("-lock"))],
    )
    .await;
}

async fn the_list_groups_by_file(c: &mut Ctx) {
    c.ui.nav("/services").await;
    c.ui.wait_for("[data-testid=file-group]", SECS).await;
    centred(c, "the group header lock").await;
    for w in [1440, 390] {
        c.ui.viewport(w, 900).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let hidden: Vec<String> = c
            .ui
            .eval("[...document.querySelectorAll('[data-testid=file-group]')].flatMap(g => { const r = g.getBoundingClientRect(); return [...g.querySelectorAll('[data-testid=group-count], [data-testid=group-problems], svg')].filter(e => { const b = e.getBoundingClientRect(); return b.width === 0 || b.right > r.right + 1 || b.left < r.left - 1; }).map(e => g.dataset.file + ' ' + (e.dataset.testid || e.tagName)); })")
            .await;
        assert!(
            hidden.is_empty(),
            "the lock, problems and count stay in view at {w}px: {hidden:?}"
        );
    }
    both_widths(
        c,
        "long-name-list",
        &[("[data-testid=file-group]", &stem("-lock"))],
    )
    .await;
}

async fn settings_files(c: &mut Ctx) {
    c.ui.nav("/settings?tab=files").await;
    c.ui.wait_for("[data-testid=files-panel]", SECS).await;
    centred(c, "the read-only tag in Settings, Files").await;
    both_widths(
        c,
        "long-name-files",
        &[("[data-testid=config-file]", &stem("-edit"))],
    )
    .await;
}

async fn editor_and_review(c: &mut Ctx) {
    start(c).await;
    let file = editable();
    go_to(c, &file, &file).await;
    both_widths(c, "long-name-editor", &[]).await;
    let ok: bool = c
        .ui
        .eval(&format!(
            "(() => {{ const i = document.querySelector('[data-testid=file-editor] [role=tabpanel][data-document-id=' + JSON.stringify({}) + '] .config-editor__input'); if (!i) return false; const at = i.value.indexOf('7778'); i.focus(); i.setSelectionRange(at, at + 4); i.setRangeText('7779', at, at + 4, 'end'); i.dispatchEvent(new InputEvent('input', {{ bubbles: true, inputType: 'insertText', data: '7779' }})); return true; }})()",
            js(&file)
        ))
        .await;
    assert!(ok, "the long-named file is open");
    tokio::time::sleep(Duration::from_millis(1000)).await;
    c.ui.click("[data-testid=file-review]").await;
    c.ui.wait_for("[data-testid=file-diff]", SECS).await;
    both_widths(
        c,
        "long-name-review",
        &[("[data-testid=file-diff] h4", &stem("-edit"))],
    )
    .await;
}

fn long_names(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        the_detail_page_keeps_its_actions(c).await;
        the_list_groups_by_file(c).await;
        settings_files(c).await;
        editor_and_review(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn long_file_names_never_break_the_layout_and_icons_sit_with_their_text() {
    run("long-names", opts(), false, long_names);
}
