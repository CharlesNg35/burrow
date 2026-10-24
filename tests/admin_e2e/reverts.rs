use std::time::Duration;

use super::edits::{Flow, click_js};
use super::guards::refused;
use super::world::Options;
use super::{Ctx, SECS, applied, edit_mode, open, open_review, run, wait_checked};

async fn activity_rev(c: &Ctx, index: usize) -> String {
    c.ui.nav("/activity").await;
    c.ui.wait_for("[data-testid=activity] [data-row]", SECS)
        .await;
    let revs =
        c.ui.all_attrs("[data-testid=activity] [data-row]", "data-row")
            .await;
    revs.get(index)
        .cloned()
        .unwrap_or_else(|| panic!("no entry {index} in {revs:?}"))
}

fn revert(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        let original = c.world.config();
        open(c, "ops").await;
        c.ui.nav("/services/web").await;
        edit_mode(c).await;
        c.ui.fill("#f-services-web-target", "4001").await;
        wait_checked(c).await;
        open_review(c, "change-to-revert").await;
        applied(c, "changed").await;
        let rev = activity_rev(c, 0).await;
        c.ui.shot("activity-with-the-change").await;
        c.ui.click(&format!("[data-row='{rev}'] [data-testid=list-row]"))
            .await;
        c.ui.wait_for("[data-testid=activity-diff]", SECS).await;
        c.ui.shot("diff-of-a-version").await;
        c.ui.click("[data-testid=revert]").await;
        c.ui.wait_for("[data-testid=review]", SECS).await;
        c.ui.shot("review-of-the-revert").await;
        applied(c, "reverted").await;
        assert_eq!(
            c.world.config(),
            original,
            "the revert restores the file byte for byte"
        );
        let at: String = c.ui.eval("location.pathname").await;
        if at.starts_with("/activity") {
            c.ui.wait_for("[data-testid=revert][disabled]", SECS).await;
        }
        c.ui.nav("/activity").await;
        c.ui.click(&format!("[data-row='{rev}'] [data-testid=list-row]"))
            .await;
        c.ui.wait_for("[data-testid=activity-diff]", SECS).await;
        assert!(
            !c.ui.enabled("[data-testid=revert]").await,
            "no revert when the file already matches that version"
        );
        c.ui.shot("a-revert-that-would-change-nothing").await;
        assert_eq!(c.world.config(), original);
        c.ui.assert_no_errors();
    })
}

#[test]
fn reverting_to_an_earlier_version_restores_the_file() {
    run("revert", Options::default(), false, revert);
}

fn revert_refused(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "boss").await;
        c.ui.nav("/visitors/ops").await;
        edit_mode(c).await;
        c.ui.choose("f-visitors-ops-admin", "read").await;
        wait_checked(c).await;
        open_review(c, "boss-demotes-ops").await;
        applied(c, "ops-is-read-only").await;
        c.ui.nav("/services/web").await;
        edit_mode(c).await;
        c.ui.fill("#f-services-web-target", "4002").await;
        wait_checked(c).await;
        open_review(c, "boss-changes-web").await;
        applied(c, "web-changed-while-ops-reads").await;

        let lab = super::world::secret();
        c.world.hand_edit(|t| {
            t.replacen(
                "# the admin\n    admin: read\n",
                "# the admin\n    admin: write\n",
                1,
            )
            .replacen(
                "clients:\n",
                &format!("clients:\n  lab:\n    secret: {lab}\n"),
                1,
            )
        });
        super::world::wait_until(
            "the hand edit that gives ops write again is running",
            Duration::from_secs(15),
            || {
                c.world
                    .status()
                    .and_then(|s| s["clients"].as_array().cloned())
                    .unwrap_or_default()
                    .iter()
                    .any(|x| x["name"] == "lab")
            },
        );

        let url = c.world.open_admin("ops");
        c.ui.goto("about:blank").await;
        c.ui.goto(&url).await;
        c.ui.wait_text("[data-testid=conn-pill]", "Connected", SECS)
            .await;
        let rev = activity_rev(c, 0).await;
        c.ui.click(&format!("[data-row='{rev}'] [data-testid=list-row]"))
            .await;
        c.ui.click("[data-testid=revert]").await;
        refused(c, "self-demoted", "revert-refused-by-the-guard").await;
        c.ui.take_errors();
    })
}

#[test]
fn a_revert_that_would_lock_me_out_is_refused() {
    run("revert-refused", Options::default(), false, revert_refused);
}

fn revert_conflict(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/services/web").await;
        edit_mode(c).await;
        c.ui.fill("#f-services-web-target", "4002").await;
        wait_checked(c).await;
        open_review(c, "a-change").await;
        applied(c, "changed").await;
        let rev = activity_rev(c, 0).await;
        c.ui.click(&format!("[data-row='{rev}'] [data-testid=list-row]"))
            .await;
        c.ui.click("[data-testid=revert]").await;
        c.ui.wait_for("[data-testid=apply]:not([disabled])", SECS)
            .await;
        c.ui.shot("revert-reviewed").await;
        c.world
            .hand_edit(|t| t.replace("target: 4002", "target: 4003"));
        c.ui.wait_for("[data-testid=banner-changed]", SECS).await;
        click_js(c, "[data-testid=apply]").await;
        c.ui.wait_for("[data-testid=conflict]", SECS).await;
        c.ui.shot("revert-against-a-changed-file").await;
        c.ui.click("[data-testid=conflict-recheck]").await;
        c.ui.wait_for("[data-testid=apply]:not([disabled])", SECS)
            .await;
        c.ui.shot("revert-reviewed-again-on-the-new-version").await;
        assert!(
            c.world.config().contains("target: 4003"),
            "the hand edit is untouched"
        );
        c.ui.take_errors();
    })
}

#[test]
fn a_revert_against_a_changed_file_is_a_conflict() {
    run(
        "revert-conflict",
        Options::default(),
        false,
        revert_conflict,
    );
}

async fn revert_review_left(c: &mut Ctx, ready: bool) {
    let rev = activity_rev(c, 0).await;
    c.ui.click(&format!("[data-row='{rev}'] [data-testid=list-row]"))
        .await;
    c.ui.wait_for("[data-testid=activity-diff]", SECS).await;
    c.ui.click("[data-testid=revert]").await;
    c.ui.wait_for("[data-testid=review]", SECS).await;
    if ready {
        c.ui.wait_for("[data-testid=apply]:not([disabled])", SECS)
            .await;
    }
}

fn stale_revert(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/services/web").await;
        edit_mode(c).await;
        c.ui.fill("#f-services-web-target", "4001").await;
        wait_checked(c).await;
        open_review(c, "change-to-revert").await;
        applied(c, "changed").await;
        revert_review_left(c, true).await;
        c.ui.shot("revert-reviewed-then-left").await;
        c.ui.nav("/clients").await;
        c.ui.click("[data-testid=new]").await;
        c.ui.fill("#new-name", "lab9").await;
        c.ui.click("[data-testid=create-apply]").await;
        c.ui.wait_for("[data-testid=shown-once]", Duration::from_secs(20))
            .await;
        c.ui.shot("create-and-apply-after-a-left-revert").await;
        let after = c.world.config();
        assert!(
            after.contains("lab9:"),
            "the new client is applied: {after}"
        );
        assert!(
            after.contains("4001"),
            "the revert was applied without a click: {after}"
        );
        c.ui.click("[data-testid=saved]").await;
        c.ui.click("[data-testid=close-shown-once]").await;
        c.ui.wait_gone("[data-testid=shown-once]", SECS).await;

        revert_review_left(c, false).await;
        c.ui.nav("/services/web").await;
        edit_mode(c).await;
        c.ui.fill("#f-services-web-target", "4002").await;
        wait_checked(c).await;
        open_review(c, "draft-review-after-a-left-revert").await;
        c.ui.wait_for("[data-testid=apply]:not([disabled])", SECS)
            .await;
        let label = c.ui.text("[data-testid=apply]").await;
        assert!(
            !label.contains("revert"),
            "the draft's review shows: {label}"
        );
        c.ui.take_errors();
    })
}

#[test]
fn a_revert_review_left_unapplied_is_never_applied_later() {
    run("stale-revert", Options::default(), false, stale_revert);
}

fn revert_after_restart(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        let original = c.world.config();
        open(c, "ops").await;
        c.ui.nav("/services/web").await;
        edit_mode(c).await;
        c.ui.fill("#f-services-web-target", "4001").await;
        wait_checked(c).await;
        open_review(c, "change-before-a-restart").await;
        applied(c, "changed").await;
        c.world.stop("server");
        c.ui.wait_text(
            "[data-testid=conn-pill]",
            "Reconnecting",
            Duration::from_secs(15),
        )
        .await;
        c.world
            .start("server", &["server", "-c", "server.yaml"], &[]);
        c.ui.wait_text(
            "[data-testid=conn-pill]",
            "Connected",
            Duration::from_secs(30),
        )
        .await;
        let older = activity_rev(c, 1).await;
        c.ui.click(&format!("[data-row='{older}'] [data-testid=list-row]"))
            .await;
        c.ui.wait_for("[data-testid=revert]", SECS).await;
        c.ui.shot("a-kept-version-after-a-restart").await;
        c.ui.click("[data-testid=revert]").await;
        c.ui.wait_for("[data-testid=review]", SECS).await;
        c.ui.shot("review-of-a-kept-version").await;
        applied(c, "reverted-after-a-restart").await;
        assert_eq!(
            c.world.config(),
            original,
            "the revert brings back the kept version"
        );
        c.ui.take_errors();
    })
}

#[test]
fn a_kept_version_can_be_reverted_after_a_server_restart() {
    run(
        "revert-after-restart",
        Options::default(),
        false,
        revert_after_restart,
    );
}

fn draft_after_a_viewed_revert_conflict(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/services/web").await;
        edit_mode(c).await;
        c.ui.fill("#f-services-web-target", "4002").await;
        wait_checked(c).await;
        open_review(c, "a-change").await;
        applied(c, "changed").await;
        revert_review_left(c, true).await;
        c.world
            .hand_edit(|t| t.replace("target: 4002", "target: 4003"));
        c.ui.wait_for("[data-testid=banner-changed]", SECS).await;
        click_js(c, "[data-testid=apply]").await;
        c.ui.wait_for("[data-testid=conflict]", SECS).await;
        c.ui.click("[data-testid=conflict-view]").await;
        c.ui.wait_gone("[data-testid=conflict]", SECS).await;
        c.ui.nav("/services/web").await;
        edit_mode(c).await;
        c.ui.fill("#f-services-web-target", "4005").await;
        wait_checked(c).await;
        c.world.hand_edit(|t| format!("{t}\n# touched\n"));
        c.ui.wait_for("[data-testid=conflict]", Duration::from_secs(15))
            .await;
        c.ui.click("[data-testid=conflict-recheck]").await;
        c.ui.wait_gone("[data-testid=conflict]", SECS).await;
        wait_checked(c).await;
        open_review(c, "review-my-draft-after-viewing-a-revert-conflict").await;
        c.ui.wait_for("[data-testid=apply]", SECS).await;
        let label = c.ui.text("[data-testid=apply]").await;
        assert!(
            !label.contains("revert"),
            "the draft's review opens: {label}"
        );
        let page = c.ui.text("[data-testid=review]").await;
        assert!(page.contains("4005"), "the draft is reviewed: {page}");
        c.ui.take_errors();
    })
}

#[test]
fn review_my_draft_opens_the_draft_after_a_revert_conflict_was_viewed() {
    run(
        "draft-after-revert-conflict",
        Options::default(),
        false,
        draft_after_a_viewed_revert_conflict,
    );
}
