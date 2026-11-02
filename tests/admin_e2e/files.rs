use std::time::Duration;

use super::edits::Flow;
use super::world::{self, Options};
use super::{Ctx, SECS, applied, both_widths, edit_mode, open, open_review, run, wait_checked};

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
                "tool.yaml",
                "# written by a tool\nservices:\n  wiki:\n    client: home\n    target: 23\n    visitors: [laptop]\n".into(),
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
    assert_eq!(c.ui.count(MENU).await, 1);
    both_widths(c, "files-read-only").await;
}

async fn the_files_tab_lists_every_file(c: &mut Ctx) {
    c.ui.nav("/settings?tab=files").await;
    c.ui.wait_for("[data-testid=files-panel]", SECS).await;
    assert_eq!(
        c.ui.all_attrs("[data-testid=config-file]", "data-file")
            .await,
        ["server.yaml", "server.d/apps.yaml", "server.d/tool.yaml"]
    );
    assert_eq!(c.ui.count("[data-testid=file-read-only]").await, 1);
    both_widths(c, "files-settings").await;
    c.ui.click("[data-file='server.d/tool.yaml'] [data-testid=file-items]")
        .await;
    c.ui.wait_for("[data-testid=entity-list]", SECS).await;
    let list = c.ui.text("[data-testid=entity-list]").await;
    assert!(
        list.contains("wiki") && !list.contains("blog"),
        "the list shows that file's items: {list}"
    );
    both_widths(c, "files-filtered-list").await;
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
        items_name_their_file(c).await;
        an_edit_stays_in_its_file(c).await;
        a_read_only_file_is_shown_not_edited(c).await;
        the_files_tab_lists_every_file(c).await;
        split_and_merge_back(c).await;
        a_new_client_in_a_file_of_its_own(c).await;
        assert_eq!(
            read(c, "server.d/tool.yaml").unwrap(),
            "# written by a tool\nservices:\n  wiki:\n    client: home\n    target: 23\n    visitors: [laptop]\n",
            "the read-only file was never written"
        );
        c.ui.assert_no_errors();
    })
}

#[test]
fn drop_in_files_are_edited_in_place_split_merged_and_read_only_ones_left_alone() {
    run("files", opts(), false, files);
}
