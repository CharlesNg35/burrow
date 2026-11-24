use std::time::Duration;

use super::text_edit::{
    ADD, CARD, CHECKED, EDITOR, REVIEW, close_of, dirty, input, js, name_new_file, replace_in,
    secrets_kept, shots, show, start, tab, text_of,
};
use super::{Ctx, SECS, adornments_clear, apply_now};

pub(super) async fn a_new_drop_in_file(c: &mut Ctx, tool_before: &str) {
    start(c).await;
    assert_eq!(
        c.ui.count(&format!("{EDITOR} .config-editor-documents__close"))
            .await,
        0,
        "the main file has no close button"
    );
    c.ui.viewport(390, 900).await;
    let reachable: bool = c
        .ui
        .eval(&format!(
            "(() => {{ const r = document.querySelector({}).getBoundingClientRect(); return r.width > 0 && r.right <= innerWidth && r.left >= 0; }})()",
            js(ADD)
        ))
        .await;
    assert!(reachable, "the + is on screen at 390px");
    c.ui.viewport(1440, 900).await;
    name_new_file(c, "my apps").await;
    c.ui.wait_text("[data-testid=file-new-problem]", "use only letters", SECS)
        .await;
    assert!(!c.ui.enabled("[data-testid=file-new-add]").await);
    c.ui.fill("#file-new-name", "apps").await;
    c.ui.wait_text("[data-testid=file-new-problem]", "already exists", SECS)
        .await;
    adornments_clear(c, "the new file's name").await;
    c.ui.shot("text-edit-new-file-name").await;
    c.ui.viewport(390, 900).await;
    adornments_clear(c, "the new file's name at 390 px").await;
    c.ui.viewport(1440, 900).await;
    c.ui.fill("#file-new-name", "extra").await;
    c.ui.wait_gone("[data-testid=file-new-problem]", SECS).await;
    c.ui.click("[data-testid=file-new-add]").await;
    let new = "server.d/extra.yaml";
    c.ui.wait_for(&input(new), SECS).await;
    c.ui.wait_for(&format!("{}[aria-selected=true]", tab(new)), SECS)
        .await;
    assert_eq!(
        c.ui.count("[data-testid=file-notes]").await,
        0,
        "an empty new file shows no note above the editor"
    );
    let panel = format!("{EDITOR} [role=tabpanel][data-document-id='{new}']");
    let top = format!(
        "document.querySelector({}).getBoundingClientRect().top",
        js(&panel)
    );
    let before: f64 = c.ui.eval(&top).await;
    replace_in(c, new, "", "s").await;
    dirty(c, new, true).await;
    let after: f64 = c.ui.eval(&top).await;
    assert!(
        (after - before).abs() < 0.5,
        "the first letter typed in a new file leaves the editor in place: {before} -> {after}"
    );
    replace_in(c, new, "s", "").await;
    c.ui.click(&close_of(new)).await;
    c.ui.wait_gone(&tab(new), SECS).await;
    assert_eq!(
        c.ui.count("[data-testid=file-close-dialog]").await,
        0,
        "an empty new file closes without asking"
    );
    name_new_file(c, "extra").await;
    c.ui.wait_gone("[data-testid=file-new-problem]", SECS).await;
    c.ui.click("[data-testid=file-new-add]").await;
    c.ui.wait_for(&input(new), SECS).await;
    replace_in(
        c,
        new,
        "",
        "services:\n  extra:\n    client: home\n    target: 2223\n    visitors: [laptop]\n",
    )
    .await;
    dirty(c, new, true).await;
    c.ui.click(&close_of(new)).await;
    c.ui.wait_for("[data-testid=file-close-dialog]", SECS).await;
    c.ui.click("[data-testid=file-close-keep]").await;
    c.ui.wait_for(&tab(new), SECS).await;
    shots(c, "text-edit-new-file").await;
    c.ui.click(REVIEW).await;
    c.ui.wait_for("[data-testid=review]", CHECKED).await;
    c.ui.wait_text(
        "[data-testid=review]",
        "server.d/extra.yaml: new file",
        SECS,
    )
    .await;
    apply_now(c, "Applied: now the version of").await;
    let made = std::fs::read_to_string(c.world.file(new)).expect("the new drop-in");
    assert!(
        made.contains("extra:") && made.contains("target: 2223"),
        "{made}"
    );
    secrets_kept(c, tool_before);
    start(c).await;
    show(c, new).await;
    assert!(
        text_of(c, new).await.contains("target: 2223"),
        "after Apply the new file is a file on the server"
    );
    dirty(c, new, false).await;
    c.ui.click(&close_of(new)).await;
    c.ui.wait_gone(&tab(new), SECS).await;
    assert_eq!(
        c.ui.count("[data-testid=file-close-dialog]").await,
        0,
        "a tab without edits closes at once; the file stays"
    );
    tokio::time::sleep(Duration::from_millis(600)).await;
    c.ui.click("[data-testid=file-cancel]").await;
    c.ui.wait_for(&format!("{CARD}[data-editing=false]"), SECS)
        .await;
}
