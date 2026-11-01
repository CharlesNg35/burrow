use std::time::Duration;

use super::edits::Flow;
use super::world::Options;
use super::{Ctx, SECS, applied, both_widths, edit_mode, open, review_now, run, wait_checked};

const MANGLE: &str = r#"(() => { if (window.__mangled) return true; window.__mangled = true; const send = WebSocket.prototype.send; WebSocket.prototype.send = function (d) { let m; try { m = JSON.parse(d); } catch (_) { return send.call(this, d); } const how = window.__mangle; if (m.t !== 'validate' || !how) return send.call(this, d); if (how === 'drop') return; if (how === 'close') { this.close(); return; } if (how === 'lock') m.body.ops.push({ op: 'set', path: ['heartbeat', 'interval_secs'], value: 11 }); if (how === 'bad') m.body.ops = 'not a list'; if (how === 'conflict') m.body.base = 'f'.repeat(64); return send.call(this, JSON.stringify(m)); }; return true; })()"#;

async fn mangle(c: &Ctx, how: &str) {
    let _: bool =
        c.ui.eval(&format!("(window.__mangle = '{how}', true)"))
            .await;
}

async fn failed(c: &Ctx, code: &str, title: &str, within: Duration) -> String {
    c.ui.wait_for(
        &format!("[data-testid=review-check-failed][data-code={code}]"),
        within,
    )
    .await;
    assert_eq!(
        c.ui.count("[data-testid=review-loading]").await,
        0,
        "a refused check does not stay on Checking"
    );
    c.ui.wait_text("[data-testid=review-check-failed]", title, SECS)
        .await;
    c.ui.text("[data-testid=check-failed-reason]").await
}

async fn again(c: &Ctx, how: &str) {
    mangle(c, how).await;
    c.ui.click("[data-testid=check-again]").await;
}

fn check_failed(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        let before = c.world.config();
        open(c, "ops").await;
        let _: bool = c.ui.eval(MANGLE).await;
        mangle(c, "lock").await;
        c.ui.nav("/services/web").await;
        edit_mode(c).await;
        c.ui.fill("#f-services-web-target", "4000").await;
        wait_checked(c).await;
        review_now(c).await;
        let refused = "The server refused this change";
        let why = failed(c, "locked-field", refused, SECS).await;
        assert!(why.contains("is locked"), "{why}");
        assert!(
            why.contains("Interval"),
            "the locked field by its label: {why}"
        );
        both_widths(c, "review-locked-field").await;
        c.ui.click("[data-testid=check-failed-back]").await;
        c.ui.wait_gone("[data-testid=review-check-failed]", SECS)
            .await;
        c.ui.wait_for("[data-testid=review-open]", SECS).await;
        review_now(c).await;
        failed(c, "locked-field", refused, SECS).await;
        again(c, "bad").await;
        let why = failed(c, "bad-request", refused, SECS).await;
        assert!(why.starts_with("Bad request body"), "{why}");
        again(c, "drop").await;
        c.ui.wait_for("[data-testid=review-loading]", SECS).await;
        let why = failed(
            c,
            "unknown",
            "The server could not check this change",
            Duration::from_secs(30),
        )
        .await;
        assert!(why.contains("did not answer within"), "{why}");
        c.ui.shot("review-check-timed-out").await;
        again(c, "close").await;
        mangle(c, "conflict").await;
        let why = failed(c, "unknown", "The server could not check this change", SECS).await;
        assert!(
            why.contains("connection closed") || why.starts_with("Not connected"),
            "{why}"
        );
        c.ui.wait_text(
            "[data-testid=conn-pill]",
            "Connected to",
            Duration::from_secs(20),
        )
        .await;
        c.ui.wait_for("[data-testid=conflict]", SECS).await;
        c.ui.click("[data-testid=conflict] button[aria-label=Close]")
            .await;
        c.ui.wait_gone("[data-testid=conflict]", SECS).await;
        c.ui.wait_for("[data-testid=review-stale]", SECS).await;
        assert_eq!(c.ui.count("[data-testid=review-loading]").await, 0);
        mangle(c, "").await;
        c.ui.click("[data-testid=review-stale] button").await;
        c.ui.wait_for("[data-testid=review]", SECS).await;
        assert_eq!(c.world.config(), before, "nothing was applied on the way");
        applied(c, "applied-after-the-refusals").await;
        assert!(c.world.config().contains("4000"));
        c.ui.take_errors();
    })
}

#[test]
fn a_refused_or_unanswered_check_shows_why_on_review_and_lets_the_user_go_back() {
    run("check-failed", Options::default(), false, check_failed);
}
