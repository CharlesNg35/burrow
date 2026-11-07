use std::time::Duration;

use super::edits::Flow;
use super::world::{self, Options};
use super::{
    Ctx, SECS, added_toasts, applied, both_widths, edit_mode, open, open_review, run, wait_checked,
};

const MENU: &str = "[data-testid=more]";

fn opts() -> Options {
    let blog = world::free_port();
    Options {
        drop_ins: vec![
            (
                "apps.yaml",
                format!(
                    "# apps, kept by hand\nservices:\n  blog:\n    client: home # the home box\n    bind: 127.0.0.1:{blog}\n    target: 22\n"
                ),
                false,
            ),
            (
                "old-server.yaml",
                "services:\n  cache:\n    client: home\n    target: 24\n    visitors: [laptop]\n"
                    .into(),
                false,
            ),
            (
                "tool.yaml",
                format!(
                    "# written by a tool\nservices:\n  wiki:\n    client: home\n    target: 23\n    visitors: [laptop]\nclients:\n  box:\n    secret: {}\n",
                    world::secret()
                ),
                true,
            ),
        ],
        ..Options::default()
    }
}

fn read(c: &Ctx, name: &str) -> Option<String> {
    std::fs::read_to_string(c.world.file(name)).ok()
}

async fn items_name_their_file(c: &mut Ctx) {
    c.ui.nav("/services/blog").await;
    let tag =
        c.ui.wait_text("[data-testid=file-tag]", "server.d/apps.yaml", SECS)
            .await;
    assert!(tag.contains("server.d/apps.yaml"), "{tag}");
    let row = c.ui.text("[data-testid=entity-list]").await;
    assert!(
        row.contains("server.d/apps.yaml"),
        "the list row names the file: {row}"
    );
    assert!(row.contains("read-only · server.d/tool.yaml"), "{row}");
    both_widths(c, "files-item-tag").await;
}

async fn an_edit_stays_in_its_file(c: &mut Ctx) {
    c.ui.nav("/services/blog").await;
    edit_mode(c).await;
    c.ui.fill("#f-services-blog-target", "2222").await;
    wait_checked(c).await;
    open_review(c, "files-review-edit").await;
    let diffs = c.ui.text("[data-testid=file-diffs]").await;
    assert!(diffs.contains("server.yaml: no change"), "{diffs}");
    assert!(diffs.contains("server.d/apps.yaml: changed"), "{diffs}");
    both_widths(c, "files-review-edit").await;
    applied(c, "files-edit-applied").await;
    let apps = read(c, "server.d/apps.yaml").unwrap();
    assert!(
        apps.contains("target: 2222") && apps.contains("# the home box"),
        "{apps}"
    );
    assert!(!c.world.config().contains("2222"));
}

async fn a_read_only_file_is_shown_not_edited(c: &mut Ctx) {
    c.ui.nav("/services/wiki").await;
    let line =
        c.ui.wait_text("[data-testid=read-only-file]", "Read-only on disk", SECS)
            .await;
    assert!(line.contains("server.d/tool.yaml"), "{line}");
    assert!(
        !c.ui.enabled("[data-testid=edit]").await,
        "Edit is off for a read-only file"
    );
    let hidden: bool = c
        .ui
        .eval("(() => { const m = document.querySelector('[data-testid=more]'); if (!m) return true; const shown = m.checkVisibility(); m.click(); return !shown; })()")
        .await;
    assert!(hidden, "the More button of a read-only item is not shown");
    tokio::time::sleep(Duration::from_millis(300)).await;
    for action in [
        "file-split",
        "file-merge",
        "file-keep",
        "rename-open",
        "delete",
    ] {
        assert_eq!(
            c.ui.count(&format!("[data-testid={action}]")).await,
            0,
            "{action} is not reachable on a read-only item"
        );
    }
    both_widths(c, "files-read-only").await;
}

async fn a_read_only_client_keeps_its_secrets(c: &mut Ctx) {
    c.ui.nav("/clients/box?tab=secrets").await;
    let list =
        c.ui.wait_text("[data-testid=secrets]", "in server.d/tool.yaml", SECS)
            .await;
    assert!(!list.contains("in server.yaml"), "{list}");
    for action in ["rotate", "remove-secret"] {
        assert_eq!(
            c.ui.count(&format!("[data-testid={action}]")).await,
            0,
            "{action} is not offered for a client in a read-only file"
        );
    }
    both_widths(c, "files-read-only-secrets").await;
    c.ui.click("[data-testid=search-open]").await;
    c.ui.fill("[data-testid=search] input", "rotate").await;
    let hits =
        c.ui.wait_text("[data-testid=search]", "Rotate secret of home", SECS)
            .await;
    assert!(!hits.contains("Rotate secret of box"), "{hits}");
    c.ui.press("[data-testid=search]", "Escape").await;
    c.ui.wait_gone("[data-testid=search]", SECS).await;
}

async fn services_of(c: &Ctx, file: &str, shown: &str) {
    let links = format!("[data-file='{file}'] [data-testid=file-items]");
    c.ui.wait_for(&links, SECS).await;
    let _: bool = c
        .ui
        .eval(&format!("(() => {{ [...document.querySelectorAll(\"{links}\")].find(b => b.innerText.includes('service')).click(); return true; }})()"))
        .await;
    c.ui.wait_text("[data-testid=entity-list]", shown, SECS)
        .await;
}

async fn the_files_tab_lists_every_file(c: &mut Ctx) {
    c.ui.nav("/settings?tab=files").await;
    c.ui.wait_for("[data-testid=files-panel]", SECS).await;
    assert_eq!(
        c.ui.all_attrs("[data-testid=config-file]", "data-file")
            .await,
        [
            "server.yaml",
            "server.d/apps.yaml",
            "server.d/old-server.yaml",
            "server.d/tool.yaml"
        ]
    );
    assert_eq!(c.ui.count("[data-testid=file-read-only]").await, 1);
    both_widths(c, "files-settings").await;
    services_of(c, "server.d/tool.yaml", "wiki").await;
    let list = c.ui.text("[data-testid=entity-list]").await;
    assert!(
        list.contains("wiki") && !list.contains("blog"),
        "the list shows that file's items: {list}"
    );
    both_widths(c, "files-filtered-list").await;
    c.ui.nav("/settings?tab=files").await;
    services_of(c, "server.yaml", "web").await;
    let list = c.ui.text("[data-testid=entity-list]").await;
    assert!(
        list.contains("web") && !list.contains("cache") && !list.contains("blog"),
        "server.yaml's link lists only its own items, not those of old-server.yaml: {list}"
    );
}

async fn split_and_merge_back(c: &mut Ctx) {
    c.ui.nav("/services/web").await;
    c.ui.click(MENU).await;
    c.ui.click("[data-testid=file-split]").await;
    open_review(c, "files-split-review").await;
    let diffs = c.ui.text("[data-testid=file-diffs]").await;
    assert!(diffs.contains("server.d/web.yaml: new file"), "{diffs}");
    let review = c.ui.text("body").await.to_lowercase();
    assert!(
        review.contains("moves service web from server.yaml to a new file, server.d/web.yaml")
            && review.contains("what changed in the files"),
        "{review}"
    );
    applied(c, "files-split-applied").await;
    let web = read(c, "server.d/web.yaml").unwrap();
    assert!(web.contains("web:"), "{web}");
    assert!(!c.world.config().contains("keep this comment"));
    c.ui.nav("/services/web").await;
    c.ui.wait_text("[data-testid=file-tag]", "server.d/web.yaml", SECS)
        .await;
    c.ui.click(MENU).await;
    c.ui.click("[data-testid=file-merge]").await;
    open_review(c, "files-merge-review").await;
    c.ui.wait_for("[data-testid=empty-file]", SECS).await;
    both_widths(c, "files-merge-offer").await;
    c.ui.click("[data-testid=drop-file]").await;
    c.ui.wait_text(
        "[data-file='server.d/web.yaml']",
        "deleted",
        Duration::from_secs(15),
    )
    .await;
    let review = c.ui.text("body").await.to_lowercase();
    assert!(review.contains("deletes server.d/web.yaml"), "{review}");
    applied(c, "files-merged").await;
    assert!(read(c, "server.d/web.yaml").is_none());
    assert!(c.world.config().contains("  web:"));
}

async fn keeping_an_item_undoes_deleting_its_file(c: &mut Ctx) {
    c.ui.nav("/services/blog").await;
    c.ui.wait_text("[data-testid=file-tag]", "server.d/apps.yaml", SECS)
        .await;
    c.ui.click(MENU).await;
    c.ui.click("[data-testid=file-merge]").await;
    open_review(c, "files-keep-review").await;
    c.ui.wait_for("[data-testid=empty-file]", SECS).await;
    c.ui.click("[data-testid=drop-file]").await;
    c.ui.wait_text(
        "[data-file='server.d/apps.yaml']",
        "deleted",
        Duration::from_secs(15),
    )
    .await;
    c.ui.nav("/services/blog").await;
    c.ui.click(MENU).await;
    c.ui.click("[data-testid=file-keep]").await;
    c.ui.wait_gone("[data-testid=review-open]", SECS).await;
    c.ui.shot("files-kept").await;
}

async fn rename_to(c: &mut Ctx, to: &str) {
    c.ui.click(MENU).await;
    c.ui.click("[data-testid=rename-open]").await;
    c.ui.wait_for("[data-testid=rename-entity] #rename-to", SECS)
        .await;
    c.ui.fill("#rename-to", to).await;
    c.ui.click("[data-testid=rename]:not([disabled])").await;
    c.ui.wait_for(&format!("[data-testid=detail][data-name={to}]"), SECS)
        .await;
}

async fn still_in_apps(c: &mut Ctx) {
    let tag =
        c.ui.wait_text("[data-testid=file-tag]", "server.d/apps.yaml", SECS)
            .await;
    assert!(tag.contains("server.d/apps.yaml"), "{tag}");
    let list = c.ui.text("[data-testid=entity-list]").await;
    assert!(list.contains("server.d/apps.yaml"), "{list}");
    c.ui.click(MENU).await;
    c.ui.wait_for("[data-testid=file-merge]", SECS).await;
    assert_eq!(c.ui.count("[data-testid=file-split]").await, 0);
}

async fn a_renamed_item_keeps_its_file(c: &mut Ctx) {
    c.ui.nav("/services/blog").await;
    rename_to(c, "journal").await;
    assert_eq!(added_toasts(c).await, 1, "a rename shows one toast");
    still_in_apps(c).await;
    both_widths(c, "files-renamed").await;
    c.ui.click(MENU).await;
    rename_to(c, "blog").await;
    still_in_apps(c).await;
    c.ui.click(MENU).await;
    c.ui.wait_gone("[data-testid=review-open]", SECS).await;
    assert_eq!(added_toasts(c).await, 0, "a rename back is no change");
}

async fn close_edit(c: &mut Ctx) {
    c.ui.click("[data-testid=save-bar] [data-testid=discard-edit]")
        .await;
    c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
}

const ROOM_AND_CARD: &str = "(() => { const c = [...document.querySelectorAll('[data-testid=detail] .rounded-card')].map(e => e.getBoundingClientRect()).filter(r => r.height > 0).pop(); return [parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--toast-room')) || 0, c.top, scrollY]; })()";

const COUNT_STACK_READS: &str = "(() => { const d = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'offsetHeight'); window.__stackReads = 0; Object.defineProperty(HTMLElement.prototype, 'offsetHeight', { configurable: true, get() { if (this.matches('[data-toast-stack]')) window.__stackReads++; return d.get.call(this); } }); window.__stackRestore = () => Object.defineProperty(HTMLElement.prototype, 'offsetHeight', d); return true; })()";

async fn the_room_holds_while_the_page_stays(c: &Ctx) {
    c.ui.viewport(420, 500).await;
    let _: bool =
        c.ui.eval("(() => { scrollTo(0, document.documentElement.scrollHeight); return true; })()")
            .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let before: Vec<f64> = c.ui.eval(ROOM_AND_CARD).await;
    assert!(before[0] > 0.0, "a toast reserves room: {before:?}");
    c.ui.wait_gone("[data-testid=toast]", Duration::from_secs(15))
        .await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    let after: Vec<f64> = c.ui.eval(ROOM_AND_CARD).await;
    assert!(
        after[0] >= before[0] && (after[1] - before[1]).abs() < 0.5,
        "the room and the page stay put when the toasts fade [room, card top, scroll]: {before:?} then {after:?}"
    );
    let _: bool = c.ui.eval(COUNT_STACK_READS).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let reads: u32 =
        c.ui.eval("(() => { window.__stackRestore(); return window.__stackReads; })()")
            .await;
    assert_eq!(
        reads, 0,
        "no timer measures the toast stack once the toasts are gone"
    );
    c.ui.viewport(1440, 900).await;
}

async fn a_new_client_after_undone_changes_shows_one_toast(c: &mut Ctx) {
    c.ui.nav("/clients").await;
    c.ui.click("[data-testid=new]").await;
    c.ui.fill("#new-name", "temp").await;
    c.ui.click("[data-testid=create]:not([disabled])").await;
    c.ui.wait_for("[data-testid=detail][data-name=temp]", SECS)
        .await;
    c.ui.wait_text("[data-testid=toasts]", "Added to changes", SECS)
        .await;
    assert_eq!(added_toasts(c).await, 1, "a new item shows one toast");
    close_edit(c).await;
    rename_to(c, "temp2").await;
    c.ui.wait_for("[data-testid=entity-list] [data-pending=new]", SECS)
        .await;
    both_widths(c, "files-renamed-new").await;
    the_room_holds_while_the_page_stays(c).await;
    close_edit(c).await;
    c.ui.wait_true(
        "getComputedStyle(document.documentElement).getPropertyValue('--toast-room') === ''",
        "the room goes once the edit ends and no toast shows",
        SECS,
    )
    .await;
    c.ui.click(MENU).await;
    c.ui.click("[data-testid=delete]").await;
    c.ui.wait_gone("[data-testid=review-open]", SECS).await;
}

async fn a_new_client_saved_in_its_own_file_counts_once(c: &mut Ctx) {
    c.ui.nav("/clients").await;
    c.ui.click("[data-testid=new]").await;
    c.ui.fill("#new-name", "rack").await;
    c.ui.select("#new-save-in", "*own").await;
    c.ui.click("[data-testid=create]:not([disabled])").await;
    c.ui.wait_for("[data-testid=detail][data-name=rack]", SECS)
        .await;
    c.ui.nav("/services/web").await;
    c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
    wait_checked(c).await;
    assert_eq!(
        c.ui.text("[data-testid=review-count]").await.trim(),
        "1",
        "a new client in a file of its own is one change"
    );
    both_widths(c, "files-new-in-own-file").await;
    open_review(c, "files-new-in-own-file-review").await;
    c.ui.wait_for("[data-testid=apply]:not([disabled])", SECS)
        .await;
    assert_eq!(
        c.ui.count("[data-testid=review-list] [data-testid=list-row]")
            .await,
        1
    );
    c.ui.click("[data-testid=discard]").await;
    c.ui.wait_gone("[data-testid=review-open]", SECS).await;
}

async fn a_new_client_in_a_file_of_its_own(c: &mut Ctx) {
    c.ui.nav("/clients").await;
    c.ui.click("[data-testid=new]").await;
    c.ui.fill("#new-name", "lab").await;
    c.ui.select("#new-save-in", "*own").await;
    both_widths(c, "files-save-in").await;
    c.ui.click("[data-testid=create-apply]").await;
    c.ui.wait_for("[data-testid=shown-once]", Duration::from_secs(20))
        .await;
    c.ui.click("[data-testid=saved]").await;
    c.ui.click("[data-testid=close-shown-once]").await;
    c.ui.wait_gone("[data-testid=shown-once]", SECS).await;
    let lab = read(c, "server.d/lab.yaml").unwrap();
    assert!(lab.starts_with("clients:\n  lab:\n"), "{lab}");
    assert!(!c.world.config().contains("  lab:"));
}

fn files(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        let tool = read(c, "server.d/tool.yaml").unwrap();
        items_name_their_file(c).await;
        an_edit_stays_in_its_file(c).await;
        a_read_only_file_is_shown_not_edited(c).await;
        a_read_only_client_keeps_its_secrets(c).await;
        the_files_tab_lists_every_file(c).await;
        split_and_merge_back(c).await;
        keeping_an_item_undoes_deleting_its_file(c).await;
        a_new_client_after_undone_changes_shows_one_toast(c).await;
        a_renamed_item_keeps_its_file(c).await;
        a_new_client_saved_in_its_own_file_counts_once(c).await;
        a_new_client_in_a_file_of_its_own(c).await;
        assert_eq!(
            read(c, "server.d/tool.yaml").unwrap(),
            tool,
            "the read-only file was never written"
        );
        c.ui.assert_no_errors();
    })
}

#[test]
fn drop_in_files_are_edited_in_place_split_merged_and_read_only_ones_left_alone() {
    run("files", opts(), false, files);
}
