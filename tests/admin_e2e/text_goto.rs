use std::time::Duration;

use super::edits::Flow;
use super::text_edit::{
    ADD, CARD, EDITOR, GOTO, MAIN, PANEL, close_of, dirty, go_to, input, js, name_new_file,
    replace_in, show, start, tab,
};
use super::text_tabs::{name, opts, tabs};
use super::{Ctx, SECS, open, run};

const LEAVE: &str = "[data-testid=file-leave-dialog]";
const SERVICES: &str = "nav[aria-label=Sections] a[href$='/services']";

async fn selected_from(c: &Ctx, file: &str, text: &str, what: &str) {
    c.ui.wait_true(
        &format!(
            "(() => {{ const i = document.querySelector({}); return !!i && document.activeElement === i && i.selectionStart === i.value.indexOf({}); }})()",
            js(&input(file)),
            js(text)
        ),
        what,
        SECS,
    )
    .await;
}

async fn exactly(c: &Ctx, file: &str, text: &str, what: &str) {
    c.ui.wait_true(
        &format!(
            "(() => {{ const i = document.querySelector({}); const at = i ? i.value.indexOf({t}) : -1; return at >= 0 && document.activeElement === i && i.selectionStart === at && i.selectionEnd === at + {t}.length; }})()",
            js(&input(file)),
            t = js(text)
        ),
        what,
        SECS,
    )
    .await;
}

async fn phone_shot(c: &mut Ctx, step: &str) {
    c.ui.viewport(390, 900).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    c.ui.shot(&format!("{step}-390")).await;
    c.ui.viewport(1440, 900).await;
}

async fn a_new_file_keeps_its_tab(c: &mut Ctx) {
    start(c).await;
    name_new_file(c, "extra").await;
    phone_shot(c, "text-goto-new-file-dialog").await;
    c.ui.click("[data-testid=file-new-add]").await;
    let new = "server.d/extra.yaml";
    c.ui.wait_for(&tab(new), SECS).await;
    for i in 1..=10 {
        go_to(c, &name(i), &name(i)).await;
        c.ui.wait_for(&format!("{}[aria-selected=true]", tab(&name(i))), SECS)
            .await;
    }
    let open = tabs(c).await;
    assert!(
        open.contains(&new.to_string()),
        "the ten-tab rule leaves an empty new file open: {open:?}"
    );
    assert!(c.ui.count(ADD).await > 0);
    c.ui.click(&close_of(new)).await;
    c.ui.wait_gone(&tab(new), SECS).await;
    assert_eq!(c.ui.count("[data-testid=file-close-dialog]").await, 0);
    replace_in(c, &name(10), "target: 3010", "target: 4010").await;
    dirty(c, &name(10), true).await;
    c.ui.click(&close_of(&name(10))).await;
    c.ui.wait_for("[data-testid=file-close-dialog]", SECS).await;
    phone_shot(c, "text-goto-close-asks").await;
    c.ui.click("[data-testid=file-close-confirm]").await;
    c.ui.wait_gone(&tab(&name(10)), SECS).await;
}

async fn matches_land_on_their_field(c: &mut Ctx) {
    show(c, MAIN).await;
    c.ui.click(GOTO).await;
    c.ui.wait_for(PANEL, SECS).await;
    let top: f64 = c.ui.eval("scrollY").await;
    go_to(c, "the home box", "# the home box").await;
    exactly(
        c,
        MAIN,
        "the home box",
        "a match in a comment selects just the match",
    )
    .await;
    assert_eq!(
        c.ui.eval::<f64>("scrollY").await,
        top,
        "jumping to a match leaves the page where it is"
    );
    go_to(c, "3005", "target: 3005").await;
    exactly(
        c,
        &name(5),
        "3005",
        "a match in a value selects just the match",
    )
    .await;
    show(c, MAIN).await;
    c.ui.select("#file-edit-as", "TOML").await;
    c.ui.wait_true(
        &format!(
            "document.querySelector({}).value.includes('max_connections = ')",
            js(&input(MAIN))
        ),
        "server.yaml shows as TOML",
        SECS,
    )
    .await;
    go_to(c, "max_connections", "max_connections: 100").await;
    selected_from(
        c,
        MAIN,
        "max_connections",
        "a match found in the YAML lands on its field in the TOML on screen",
    )
    .await;
}

async fn leave_dialog(c: &Ctx) {
    c.ui.wait_for(LEAVE, SECS).await;
    c.ui.wait_text(
        LEAVE,
        "server.yaml has TOML text that cannot be saved as YAML yet.",
        SECS,
    )
    .await;
    c.ui.wait_true(
        "!!document.activeElement && document.activeElement.dataset.testid === 'file-leave-stay'",
        "Stay is the default",
        SECS,
    )
    .await;
}

async fn still_here(c: &Ctx) {
    c.ui.click("[data-testid=file-leave-stay]").await;
    c.ui.wait_gone(LEAVE, SECS).await;
    c.ui.wait_for(&format!("{CARD}[data-editing=true]"), SECS)
        .await;
    assert!(
        c.ui.eval::<String>(&format!(
            "document.querySelector({}).value",
            js(&input(MAIN))
        ))
        .await
        .contains("broken = ["),
        "the text on screen is still there"
    );
}

async fn leaving_unwritten_text_asks(c: &mut Ctx) {
    let last: String =
        c.ui.eval(&format!(
            "document.querySelector({}).value.trimEnd().split('\\n').pop()",
            js(&input(MAIN))
        ))
        .await;
    replace_in(c, MAIN, &last, &format!("{last}\nbroken = [")).await;
    dirty(c, MAIN, true).await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    c.ui.click("[data-testid=file-cancel]").await;
    c.ui.wait_for("[data-testid=file-cancel-dialog]", SECS)
        .await;
    c.ui.click("[data-testid=file-keep]").await;
    c.ui.click("[data-testid=settings-tabs] [role=tab][aria-selected=true]")
        .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        c.ui.count(LEAVE).await,
        0,
        "the tab already shown goes nowhere, so nothing asks"
    );
    c.ui.click(SERVICES).await;
    leave_dialog(c).await;
    phone_shot(c, "text-goto-leave").await;
    c.ui.shot("text-goto-leave").await;
    still_here(c).await;
    c.ui.click("[data-testid=settings-tabs] [role=tab]").await;
    leave_dialog(c).await;
    still_here(c).await;
    let _: bool =
        c.ui.eval("(() => { history.back(); return true; })()")
            .await;
    leave_dialog(c).await;
    still_here(c).await;
    assert!(
        c.ui.eval::<String>("location.search")
            .await
            .contains("tab=file"),
        "Stay keeps the address of the Config file tab"
    );
    let _: bool = c
        .ui
        .eval("(() => { document.querySelector('a[href=\"#main\"]').click(); return true; })()")
        .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        c.ui.count(LEAVE).await,
        0,
        "the skip link goes nowhere, so nothing asks"
    );
    c.ui.click(SERVICES).await;
    c.ui.wait_for(LEAVE, SECS).await;
    c.ui.click("[data-testid=file-leave-confirm]").await;
    c.ui.wait_true(
        "location.pathname.includes('/services')",
        "Leave goes where the user asked",
        SECS,
    )
    .await;
    c.ui.nav("/settings?tab=file").await;
    c.ui.wait_for(&format!("{CARD}[data-editing=true]"), SECS)
        .await;
    c.ui.click("[data-testid=file-cancel]").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    if c.ui.count("[data-testid=file-cancel-dialog]").await > 0 {
        c.ui.click("[data-testid=file-discard]").await;
    }
    c.ui.wait_for(&format!("{CARD}[data-editing=false]"), SECS)
        .await;
    assert!(c.ui.count(EDITOR).await == 0);
}

async fn a_file_removed_on_the_server_takes_its_edits(c: &mut Ctx) {
    let gone = name(12);
    show(c, &gone).await;
    replace_in(c, &gone, "target: 3012", "target: 4012").await;
    dirty(c, &gone, true).await;
    std::fs::remove_file(c.world.file(&gone)).expect("remove the drop-in");
    c.ui.wait_text(
        "body",
        "server.d/web-12.yaml was removed on the server; its edits here are gone",
        Duration::from_secs(20),
    )
    .await;
    c.ui.wait_gone(&tab(&gone), SECS).await;
    c.ui.wait_true(
        "document.querySelector('[data-testid=file-review]').disabled",
        "nothing left to review",
        SECS,
    )
    .await;
    c.ui.click("[data-testid=file-cancel]").await;
    c.ui.wait_for(&format!("{CARD}[data-editing=false]"), SECS)
        .await;
    c.ui.click("[data-testid=file-edit]").await;
    c.ui.wait_for(&input(MAIN), SECS).await;
}

fn text_goto(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        a_new_file_keeps_its_tab(c).await;
        a_file_removed_on_the_server_takes_its_edits(c).await;
        matches_land_on_their_field(c).await;
        leaving_unwritten_text_asks(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn go_to_lands_on_fields_in_any_language_and_leaving_unwritten_text_asks() {
    run("text-goto", opts(), false, text_goto);
}
