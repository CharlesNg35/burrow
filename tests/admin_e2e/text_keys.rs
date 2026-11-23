use super::edits::Flow;
use super::text_edit::{EDITOR, MAIN, input, js, key, replace_in, show, start, tab};
use super::world::{self, Options};
use super::{Ctx, SECS, open, run};

const APPS: &str = "server.d/apps.yaml";
const TOOL: &str = "server.d/tool.yaml";
const MORE: &str = "server.d/more.yaml";
const ROWS: &str = ".config-editor__completion";

fn opts() -> Options {
    Options {
        drop_ins: vec![
            (
                "apps.yaml",
                "services:\n  blog:\n    client: home\n    target: 22\n    visitors: [laptop]\n"
                    .into(),
                false,
            ),
            (
                "tool.yaml",
                format!("clients:\n  box:\n    secret: {}\n", world::secret()),
                false,
            ),
            (
                "more.yaml",
                "services:\n  shell:\n    client: box\n    target: 22\n".into(),
                false,
            ),
        ],
        ..Options::default()
    }
}

async fn offered(c: &Ctx, name: &str) -> Vec<(String, String)> {
    key(c, name, " ", "Space", "ctrlKey: true,").await;
    let rows = format!("{EDITOR} {ROWS}");
    c.ui.wait_for(&rows, SECS).await;
    c.ui.eval(&format!(
        "[...document.querySelectorAll({})].map(r => [r.querySelector('.config-editor__completion-key')?.textContent.trim() ?? '', r.querySelector('.config-editor__completion-detail')?.textContent.trim() ?? ''])",
        js(&rows)
    ))
    .await
}

fn has(rows: &[(String, String)], name: &str, from: &str) -> bool {
    rows.iter().any(|(k, d)| k == name && d.contains(from))
}

fn named(rows: &[(String, String)], name: &str) -> bool {
    rows.iter().any(|(k, _)| k == name)
}

async fn reach_keys(c: &Ctx) {
    show(c, MAIN).await;
    let had = "    reach:\n      home: [127.0.0.1/32]\n";
    replace_in(c, MAIN, had, &format!("{had}      ")).await;
    let rows = offered(c, MAIN).await;
    assert!(
        has(&rows, "fresh", "this document") && has(&rows, "box", TOOL) && named(&rows, "self"),
        "a new key of a visitor's reach is offered the client names: {rows:?}"
    );
    assert!(
        !named(&rows, "home"),
        "but not the clients reach has already: {rows:?}"
    );
    key(c, MAIN, "Escape", "Escape", "").await;
    replace_in(c, MAIN, &format!("{had}      "), had).await;
}

async fn names_across_files(c: &mut Ctx) {
    start(c).await;
    replace_in(c, MAIN, "  home:\n", "  fresh:\n    secret: x\n  home:\n").await;
    show(c, APPS).await;
    replace_in(c, APPS, "client: home", "client: ").await;
    let rows = offered(c, APPS).await;
    assert!(
        has(&rows, "fresh", MAIN),
        "a client typed in server.yaml, not saved, is offered at once: {rows:?}"
    );
    assert!(
        has(&rows, "box", TOOL),
        "a client of a file not open is offered, with its file: {rows:?}"
    );
    assert!(named(&rows, "self"), "and the word self: {rows:?}");
    c.ui.view_shot("text-keys-offered").await;
    key(c, APPS, "Escape", "Escape", "").await;
    replace_in(c, APPS, "visitors: [laptop]", "visitors: [laptop, ]").await;
    let at: bool = c
        .ui
        .eval(&format!(
            "(() => {{ const i = document.querySelector({}); const at = i.value.indexOf('laptop, ') + 8; i.focus(); i.setSelectionRange(at, at); return true; }})()",
            js(&input(APPS))
        ))
        .await;
    assert!(at);
    let rows = offered(c, APPS).await;
    assert!(
        named(&rows, "ops") && !named(&rows, "laptop"),
        "a list of visitors offers the visitors not in it yet: {rows:?}"
    );
    key(c, APPS, "Escape", "Escape", "").await;
    replace_in(c, APPS, "visitors: [laptop, ]", "visitors: [laptop]").await;
    reach_keys(c).await;
    show(c, APPS).await;
    replace_in(c, APPS, "client: \n", "client: box\n").await;
    let on: bool = c
        .ui
        .eval(&format!(
            "(() => {{ const i = document.querySelector({}); const at = i.value.indexOf('client: box') + 9; i.focus(); i.setSelectionRange(at, at); i.dispatchEvent(new Event('select', {{ bubbles: true }})); i.dispatchEvent(new KeyboardEvent('keyup', {{ key: 'ArrowRight', bubbles: true }})); return true; }})()",
            js(&input(APPS))
        ))
        .await;
    assert!(on);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    key(c, APPS, "F12", "F12", "").await;
    c.ui.wait_for(&format!("{}[aria-selected=true]", tab(TOOL)), SECS)
        .await;
    c.ui.wait_true(
        &format!(
            "(() => {{ const i = document.querySelector({}); return !!i && i.value.slice(0, i.selectionStart).endsWith('clients:\\n  '); }})()",
            js(&input(TOOL))
        ),
        "F12 on a name another file defines opens that file at its key",
        SECS,
    )
    .await;
    uses_across_files(c).await;
}

async fn uses_across_files(c: &Ctx) {
    key(c, TOOL, "F12", "F12", "shiftKey: true,").await;
    let group = |file: &str| {
        format!(
            "{EDITOR} .config-editor__uses-group[aria-label={}]",
            js(file)
        )
    };
    c.ui.wait_for(&group(APPS), SECS).await;
    c.ui.wait_for(&group(MORE), SECS).await;
    c.ui.view_shot("text-keys-uses").await;
    c.ui.click(&format!("{} .config-editor__use", group(MORE)))
        .await;
    c.ui.wait_for(&format!("{}[aria-selected=true]", tab(MORE)), SECS)
        .await;
}

fn text_keys(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        names_across_files(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_name_is_offered_from_every_file_unsaved_edits_included() {
    run("text-keys", opts(), false, text_keys);
}
