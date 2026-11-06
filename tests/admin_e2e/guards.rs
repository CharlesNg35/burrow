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

pub(super) async fn confirm_asked(c: &mut Ctx, code: &str, word: &str, step: &str) {
    c.ui.wait_for(&format!("[data-impact={code}][data-level=danger]"), SECS)
        .await;
    c.ui.wait_for(
        &format!("[data-testid=confirms] [data-confirm={word}]"),
        SECS,
    )
    .await;
    assert_eq!(
        c.ui.count("[data-refusal]").await,
        0,
        "{code} is no refusal"
    );
    assert!(
        !c.ui.enabled("[data-testid=apply]").await,
        "Apply waits for the typed {word}"
    );
    super::letsencrypt::three(c, step).await;
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
        confirm_asked(c, "self-demoted", "ops", "confirm-self-demoted").await;
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
fn e7_removing_yourself_is_refused_losing_your_access_is_confirmed_and_read_access_cannot_edit() {
    run("e7-lockout", Options::default(), false, lockout);
}

fn admin_rights(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "boss").await;
        c.ui.nav("/visitors/viewer").await;
        edit_mode(c).await;
        assert_eq!(c.ui.count("[data-testid=grant-lock]").await, 0);
        c.ui.choose("f-visitors-viewer-admin", "write").await;
        assert!(
            c.ui.eval::<bool>(
                "document.querySelector(\"#f-visitors-viewer-admin input[value='write']\").checked"
            )
            .await,
            "read becomes write"
        );
        c.ui.click("[data-testid=save]").await;
        c.ui.wait_gone("[data-testid=save-bar]", SECS).await;

        c.ui.nav("/visitors/plain").await;
        edit_mode(c).await;
        c.ui.choose("f-visitors-plain-admin", "read").await;
        c.ui.wait_for("#f-visitors-plain-admin-from", SECS).await;
        c.ui.click("[data-testid=save]").await;
        c.ui.wait_gone("[data-testid=save-bar]", SECS).await;

        c.ui.nav("/visitors/ops").await;
        edit_mode(c).await;
        c.ui.fill("#f-visitors-ops-admin-from", "10.0.0.0/8").await;
        c.ui.wait_for(
            "[data-schema='visitors/*/admin_from'] [data-chip='10.0.0.0/8']",
            SECS,
        )
        .await;
        wait_checked(c).await;
        c.ui.shot("admin-from-widened").await;
        c.ui.click("[data-testid=save]").await;
        c.ui.wait_gone("[data-testid=save-bar]", SECS).await;

        c.ui.nav("/visitors/boss?tab=secrets").await;
        assert!(
            c.ui.enabled("[data-testid=rotate]").await,
            "an admin rotates its own secret here"
        );
        c.ui.click("[data-testid=more]").await;
        c.ui.wait_for("[data-testid=more-menu] [data-testid=rename-open]", SECS)
            .await;
        assert!(
            c.ui.enabled("[data-testid=rename-open]").await,
            "an admin visitor is renamed like any other"
        );
        c.ui.click("[data-testid=more]").await;

        open_review(c, "admin-rights-widened").await;
        assert_eq!(c.ui.count("[data-refusal]").await, 0);
        super::letsencrypt::three(c, "admin-rights-review").await;
        applied(c, "admin-rights-applied").await;
        let text = c.world.config();
        assert!(!text.contains("admin: read\n  laptop"), "{text}");
        assert!(
            text.contains("  plain:\n") && text.contains("admin: read"),
            "{text}"
        );
        assert!(
            text.contains("admin_from: [127.0.0.1/32, 10.0.0.0/8]"),
            "{text}"
        );
        c.ui.assert_no_errors();
    })
}

#[test]
fn admin_rights_are_raised_and_widened_in_the_visitor_form() {
    run("admin-rights", Options::default(), false, admin_rights);
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
