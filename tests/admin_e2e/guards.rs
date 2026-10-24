use std::time::Duration;

use super::edits::{Flow, UNSET, discard_from_review};
use super::world::Options;
use super::{Ctx, SECS, applied, echo_server, edit_mode, open, open_review, run, wait_checked};

fn conflict(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/services/web").await;
        edit_mode(c).await;
        c.ui.fill("#f-services-web-target", "4000").await;
        wait_checked(c).await;
        c.ui.click("[data-testid=save]").await;
        c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
        c.ui.shot("editing-web-target").await;
        let lab = super::world::secret();
        c.world.hand_edit(|t| {
            t.replacen(
                "clients:\n",
                &format!("clients:\n  lab:\n    secret: {lab}\n"),
                1,
            )
        });
        c.ui.wait_for("[data-testid=conflict]", Duration::from_secs(15))
            .await;
        c.ui.wait_text(
            "[data-testid=conflict-theirs]",
            "New client lab (1 secret)",
            SECS,
        )
        .await;
        c.ui.shot("conflict-dialog").await;
        c.ui.click("[data-testid=conflict] button[aria-label=Close]")
            .await;
        c.ui.wait_gone("[data-testid=conflict]", SECS).await;
        tokio::time::sleep(Duration::from_secs(6)).await;
        assert_eq!(
            c.ui.count("[data-testid=banner-changed]").await,
            1,
            "closing the dialog keeps the banner while the draft is on the old rev"
        );
        assert_eq!(
            c.ui.count("[data-testid=conflict]").await,
            0,
            "the closed dialog does not come back on its own"
        );
        c.ui.click("[data-testid=review-open]").await;
        c.ui.wait_for("[data-testid=review-stale]", SECS).await;
        c.ui.shot("review-on-the-old-rev").await;
        c.ui.click("[data-testid=review-stale] button").await;
        c.ui.wait_gone("[data-testid=review-stale]", SECS).await;
        c.ui.wait_for("[data-testid=review]", SECS).await;
        assert_eq!(
            c.ui.count("[data-testid=banner-changed]").await,
            0,
            "the banner goes with the draft it protected"
        );
        wait_checked(c).await;
        c.ui.shot("draft-kept-on-the-new-version").await;
        open_review(c, "review-after-recheck").await;
        applied(c, "applied-on-the-new-version").await;
        let text = c.world.config();
        assert!(text.contains("lab:"), "the hand edit stays:\n{text}");
        assert!(text.contains("target: 4000"), "my change is in:\n{text}");
        c.ui.assert_no_errors();
    })
}

#[test]
fn e5_a_hand_edit_during_editing_is_a_conflict_and_the_draft_is_kept() {
    run("e5-conflict", Options::default(), false, conflict);
}

pub(super) async fn refused(c: &mut Ctx, code: &str, step: &str) {
    c.ui.wait_for(&format!("[data-refusal={code}]"), SECS).await;
    assert!(
        !c.ui.enabled("[data-testid=apply]").await,
        "no Apply for a lockout"
    );
    c.ui.wait_text("[data-testid=apply]", "Apply refused", SECS)
        .await;
    c.ui.shot(step).await;
}

fn lockout(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        let before = c.world.config();
        open(c, "ops").await;
        c.ui.nav("/visitors/ops").await;
        c.ui.shot("ops-visitor").await;
        edit_mode(c).await;
        c.ui.choose("f-visitors-ops-admin", UNSET).await;
        wait_checked(c).await;
        open_review(c, "removing-my-admin-right").await;
        refused(c, "self-demoted", "refused-self-demoted").await;
        c.ui.wait_for("[data-testid=fix-undo]", SECS).await;
        discard_from_review(c).await;

        c.ui.nav("/visitors/laptop").await;
        c.ui.nav("/visitors/ops").await;
        c.ui.click("[data-testid=more]").await;
        c.ui.click("[data-testid=delete]").await;
        wait_checked(c).await;
        c.ui.nav("/visitors/ops").await;
        c.ui.wait_for("[data-testid=undo-remove]", SECS).await;
        let gap: f64 = c
            .ui
            .eval("(() => { const a = document.querySelector('[data-testid=undo-remove]').getBoundingClientRect(); const b = document.querySelector('[data-testid=detail-missing] a').getBoundingClientRect(); return b.left >= a.right ? b.left - a.right : b.top - a.bottom; })()")
            .await;
        assert!(gap >= 6.0, "buttons side by side keep a gap: {gap}");
        c.ui.shot("ops-deleted-in-draft").await;
        open_review(c, "deleting-myself").await;
        refused(c, "self-removed", "refused-self-removed").await;
        discard_from_review(c).await;
        assert_eq!(c.world.config(), before, "server.yaml is unchanged");

        let url = c.world.open_admin("viewer");
        c.ui.goto("about:blank").await;
        c.ui.goto(&url).await;
        c.ui.wait_for("[data-testid=read-only-tag]", SECS).await;
        c.ui.nav("/services/web?tab=settings").await;
        c.ui.wait_for("[data-testid=detail]", SECS).await;
        c.ui.wait_text(
            "[data-schema='services/*/target']",
            &c.world.ports["echo"].to_string(),
            SECS,
        )
        .await;
        assert_eq!(
            c.ui.count("[data-editor]").await,
            0,
            "read access sees plain values, not controls that invite clicks"
        );
        assert_eq!(
            c.ui.count("[data-testid=save-bar]").await,
            0,
            "the read-only state is told once, by the header tag"
        );
        assert!(
            !c.ui.enabled("[data-testid=edit]").await,
            "Edit is disabled for read access"
        );
        let more: bool =
            c.ui.eval("document.querySelector('[data-testid=more]').offsetParent !== null")
                .await;
        assert!(!more, "read access gets no More menu");
        c.ui.nav("/services").await;
        assert!(
            !c.ui.enabled("[data-testid=new]").await,
            "+ is disabled for read access"
        );
        c.ui.shot("viewer-cannot-edit").await;
        c.ui.take_errors();
    })
}

#[test]
fn e7_the_lockout_guard_refuses_and_read_access_cannot_edit() {
    run("e7-lockout", Options::default(), false, lockout);
}

const IN_THE_FILE: &str = "Admin access is granted in the config file (burrow edit)";

async fn off(c: &Ctx, visitor: &str, level: &str) -> bool {
    c.ui.count(&format!(
        "#f-visitors-{visitor}-admin label[data-off=true] input[value='{level}']"
    ))
    .await
        == 1
}

fn grant_lock(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "boss").await;
        c.ui.nav("/visitors/viewer").await;
        edit_mode(c).await;
        c.ui.wait_text(
            "[data-schema='visitors/*/admin'] [data-testid=grant-lock]",
            IN_THE_FILE,
            SECS,
        )
        .await;
        assert!(off(c, "viewer", "write").await, "read cannot become write");
        assert!(!off(c, "viewer", "read").await);
        c.ui.choose("f-visitors-viewer-admin", "write").await;
        assert!(
            !c.ui.eval::<bool>("document.querySelector(\"#f-visitors-viewer-admin input[value='write']\").checked")
                .await,
            "a refused level is not chosen"
        );
        c.ui.shot("viewer-may-only-go-down").await;

        c.ui.nav("/visitors/plain").await;
        edit_mode(c).await;
        assert_eq!(
            c.ui.count("#f-visitors-plain-admin input").await,
            0,
            "no selector to offer when nothing may be chosen"
        );
        c.ui.wait_for(
            "[data-schema='visitors/*/admin'] [data-testid=grant-lock]",
            SECS,
        )
        .await;
        c.ui.shot("no-admin-for-a-visitor-without-it").await;

        c.ui.nav("/visitors/boss?tab=secrets").await;
        c.ui.wait_for("[data-testid=secrets] [data-testid=grant-lock]", SECS)
            .await;
        assert!(
            !c.ui.enabled("[data-testid=rotate]").await,
            "an admin gets a new secret only in the file"
        );
        c.ui.shot("admin-secret-locked").await;
        c.ui.click("[data-testid=more]").await;
        c.ui.wait_for("[data-testid=more-menu] [data-testid=grant-lock]", SECS)
            .await;
        assert!(
            !c.ui.enabled("[data-testid=rename-open]").await,
            "an admin visitor is renamed only in the file"
        );
        c.ui.shot("admin-rename-locked").await;
        c.ui.click("[data-testid=more]").await;
        c.ui.nav("/visitors/plain").await;
        c.ui.click("[data-testid=more]").await;
        c.ui.wait_for("[data-testid=more-menu] [data-testid=rename-open]", SECS)
            .await;
        assert!(c.ui.enabled("[data-testid=rename-open]").await);
        assert_eq!(
            c.ui.count("[data-testid=more-menu] [data-testid=grant-lock]")
                .await,
            0
        );
        c.ui.click("[data-testid=more]").await;

        c.ui.nav("/visitors/ops").await;
        edit_mode(c).await;
        let from = "#f-visitors-ops-admin-from";
        let chips = "[data-schema='visitors/*/admin_from'] [data-chip]";
        c.ui.click("[data-schema='visitors/*/admin_from'] [data-chip='127.0.0.1/32'] button")
            .await;
        c.ui.wait_text(
            "[data-schema='visitors/*/admin_from'] [data-testid=grant-lock]",
            IN_THE_FILE,
            SECS,
        )
        .await;
        assert_eq!(c.ui.count(chips).await, 1, "the last entry stays");
        c.ui.fill(from, "10.0.0.0/8").await;
        c.ui.wait_for(
            "[data-schema='visitors/*/admin_from'] [data-testid=grant-lock]",
            SECS,
        )
        .await;
        assert_eq!(c.ui.count(chips).await, 1, "a wider entry is not added");
        c.ui.shot("admin-from-may-not-widen").await;
        c.ui.fill(from, "127.0.0.1").await;
        c.ui.wait_for(
            "[data-schema='visitors/*/admin_from'] [data-chip='127.0.0.1']",
            SECS,
        )
        .await;
        c.ui.wait_gone(
            "[data-schema='visitors/*/admin_from'] [data-testid=grant-lock]",
            SECS,
        )
        .await;
        wait_checked(c).await;
        c.ui.click("[data-testid=save]").await;
        c.ui.wait_gone("[data-testid=save-bar]", SECS).await;

        c.ui.nav("/visitors/viewer").await;
        edit_mode(c).await;
        c.ui.choose("f-visitors-viewer-admin", UNSET).await;
        wait_checked(c).await;
        open_review(c, "viewer-loses-admin").await;
        applied(c, "viewer-is-no-admin").await;
        let text = c.world.config();
        assert!(!text.contains("admin: read"), "{text}");
        assert!(
            text.contains("admin_from: [127.0.0.1/32, 127.0.0.1]"),
            "{text}"
        );
        c.ui.assert_no_errors();
    })
}

#[test]
fn admin_power_is_only_lowered_in_the_visitor_form() {
    run("grant-lock", Options::default(), false, grant_lock);
}

fn typed_confirm(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        let echo = echo_server(c.world.ports["echo"]).await;
        let port = c.world.ports["web"];
        let held = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect");
        open(c, "ops").await;
        c.ui.nav("/services/web").await;
        c.ui.wait_not_text("[data-testid=live] [data-stat=open]", "0", SECS)
            .await;
        c.ui.shot("web-with-an-open-connection").await;
        c.ui.click("[data-testid=more]").await;
        c.ui.click("[data-testid=delete]").await;
        wait_checked(c).await;
        open_review(c, "impact-of-removing-web").await;
        let impacts = c.ui.text("[data-testid=review]").await;
        assert!(impacts.contains("open connection"), "{impacts}");
        c.ui.wait_for("[data-impact][data-level=danger]", SECS)
            .await;
        c.ui.wait_for("[data-testid=confirms] [data-confirm=web]", SECS)
            .await;
        assert!(
            !c.ui.enabled("[data-testid=apply]").await,
            "Apply waits for the typed name"
        );
        c.ui.fill("[data-confirm=web]", "we").await;
        assert!(!c.ui.enabled("[data-testid=apply]").await);
        c.ui.shot("typed-confirmation-incomplete").await;
        c.ui.fill("[data-confirm=web]", "web").await;
        c.ui.shot("typed-confirmation-done").await;
        applied(c, "web-removed").await;
        assert!(!c.world.config().contains("  web:"));
        drop(held);
        echo.abort();
        c.ui.assert_no_errors();
    })
}

#[test]
fn removing_a_busy_service_shows_its_impact_and_needs_its_name_typed() {
    run("typed-confirm", Options::default(), false, typed_confirm);
}
