use std::time::{Duration, SystemTime};

use super::edits::Flow;
use super::text_edit::{
    CARD, MAIN, REVIEW, click_toast, dirty, input, js, replace_in, show, start, tab,
};
use super::text_tabs::{FILES, MAX_TABS, name, opts, tabs};
use super::{Ctx, SECS, apply_now, open, pick_in, run};

const LAB: &str = "server.d/lab.yaml";
const TOAST: &str = "[data-testid=toasts]";

fn age_drop_ins(c: &Ctx) {
    let base = SystemTime::now() - Duration::from_secs(3600);
    for i in 1..=FILES {
        let file = std::fs::File::options()
            .write(true)
            .open(c.world.file(&name(i)))
            .expect("a drop-in");
        file.set_modified(base + Duration::from_secs(i as u64))
            .expect("a change time");
    }
}

async fn shown(c: &Ctx, file: &str) {
    c.ui.wait_for(&format!("{}[aria-selected=true]", tab(file)), SECS)
        .await;
}

async fn cancel(c: &Ctx) {
    c.ui.click("[data-testid=file-cancel]").await;
    c.ui.wait_for(&format!("{CARD}[data-editing=false]"), SECS)
        .await;
}

fn newest(first: &[&str], from: usize) -> Vec<String> {
    let mut out: Vec<String> = first.iter().map(|s| s.to_string()).collect();
    out.extend((1..=from).rev().map(name));
    out.truncate(MAX_TABS);
    out
}

async fn raw_config_opens_with_the_newest_files(c: &mut Ctx) {
    age_drop_ins(c);
    start(c).await;
    assert_eq!(
        tabs(c).await,
        newest(&[MAIN], FILES),
        "the main file first, then the files changed last"
    );
    shown(c, MAIN).await;
    cancel(c).await;
}

async fn hold_toast(c: &Ctx, on: bool) {
    let ev = if on { "mouseenter" } else { "mouseleave" };
    let _: bool = c
        .ui
        .eval(&format!(
            "(() => {{ for (const t of document.querySelectorAll('[data-testid=toast]')) t.dispatchEvent(new MouseEvent({}, {{ bubbles: false }})); return true; }})()",
            js(ev)
        ))
        .await;
}

async fn the_apply_names_the_file_and_opens_it(c: &mut Ctx) {
    c.ui.nav("/clients").await;
    c.ui.click("[data-testid=new]").await;
    c.ui.fill("#new-name", "lab").await;
    pick_in(c, "new-save-in", "*own").await;
    c.ui.click("[data-testid=create-apply]").await;
    c.ui.wait_text(TOAST, "saved in server.d/lab.yaml", Duration::from_secs(20))
        .await;
    hold_toast(c, true).await;
    c.ui.wait_for("[data-testid=shown-once]", SECS).await;
    c.ui.click("[data-testid=saved]").await;
    c.ui.click("[data-testid=close-shown-once]").await;
    c.ui.wait_gone("[data-testid=shown-once]", SECS).await;
    let note = c.ui.text(TOAST).await;
    assert!(
        note.contains("Applied: now the version of")
            && note.contains(", saved in server.d/lab.yaml"),
        "the message keeps the version and names the file: {note}"
    );
    c.ui.shot("apply-names-file").await;
    click_toast(c, "Open").await;
    hold_toast(c, false).await;
    c.ui.wait_for(&format!("{CARD}[data-editing=true]"), SECS)
        .await;
    shown(c, LAB).await;
    c.ui.wait_true(
        &format!(
            "(() => {{ const i = document.querySelector({}); return !!i && document.activeElement === i && i.selectionStart === i.value.indexOf('lab:'); }})()",
            js(&input(LAB))
        ),
        "Open lands on the new client",
        SECS,
    )
    .await;
    assert_eq!(tabs(c).await, newest(&[MAIN, LAB], FILES));
    cancel(c).await;
}

async fn raw_config_opens_on_the_file_of_the_last_apply(c: &mut Ctx) {
    start(c).await;
    shown(c, LAB).await;
    assert_eq!(
        tabs(c).await,
        newest(&[MAIN, LAB], FILES),
        "the file just written comes first after the main file"
    );
    c.ui.shot("raw-config-filled-tabs").await;
    show(c, MAIN).await;
    replace_in(c, MAIN, "max_connections: 100", "max_connections: 101").await;
    dirty(c, MAIN, true).await;
    c.ui.wait_for(&format!("{REVIEW}:not([disabled])"), SECS)
        .await;
    c.ui.click(REVIEW).await;
    c.ui.wait_for("[data-testid=review]", Duration::from_secs(15))
        .await;
    apply_now(c, "Applied: now the version of").await;
    let note = c.ui.text(TOAST).await;
    assert!(
        !note.contains("saved in"),
        "the main file goes unnamed: {note}"
    );
    start(c).await;
    shown(c, MAIN).await;
    cancel(c).await;
}

fn text_landed(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        raw_config_opens_with_the_newest_files(c).await;
        the_apply_names_the_file_and_opens_it(c).await;
        raw_config_opens_on_the_file_of_the_last_apply(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn raw_config_opens_the_newest_files_and_the_file_an_apply_wrote() {
    run("text-landed", opts(), false, text_landed);
}
