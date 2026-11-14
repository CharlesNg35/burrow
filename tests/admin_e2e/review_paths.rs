use super::edits::Flow;
use super::world::Options;
use super::{Ctx, SECS, both_widths, edit_mode, open, open_review, run};

const LINE: &str = "[data-testid=review-path-line]";

async fn change_target(c: &Ctx, service: &str, port: &str) {
    c.ui.nav(&format!("/services/{service}")).await;
    edit_mode(c).await;
    c.ui.fill(&format!("#f-services-{service}-target"), port)
        .await;
}

async fn opened(c: &Ctx) -> Vec<bool> {
    c.ui.eval("[...document.querySelectorAll('[data-review-path]')].map(d => d.tagName === 'DETAILS' ? d.open : true)")
        .await
}

async fn one_service_has_its_diagram_without_a_fold(c: &mut Ctx) {
    change_target(c, "web", "4001").await;
    open_review(c, "review-paths-one").await;
    assert_eq!(c.ui.count("[data-review-path]").await, 1);
    assert_eq!(c.ui.count(LINE).await, 0, "one service has no summary line");
    assert_eq!(
        c.ui.count("[data-review-path] details, details[data-review-path]")
            .await,
        0
    );
    c.ui.click("[data-testid=keep-editing]").await;
    c.ui.wait_gone("[data-testid=review]", SECS).await;
}

async fn several_services_are_one_line_each(c: &mut Ctx) {
    change_target(c, "ssh", "2223").await;
    open_review(c, "review-paths-two").await;
    assert_eq!(c.ui.count(LINE).await, 2, "one line per changed service");
    assert_eq!(opened(c).await, [false, false], "the lines start folded");
    let lines: Vec<String> = c
        .ui
        .eval("[...document.querySelectorAll('[data-testid=review-path-line]')].map(l => l.innerText.replace(/\\s+/g, ' ').trim())")
        .await;
    assert!(
        lines[0].starts_with("ssh") && lines[0].contains("target") && lines[0].contains("changed"),
        "{lines:?}"
    );
    assert!(
        lines[0].contains("→") && lines[0].contains("2223"),
        "{lines:?}"
    );
    assert!(
        lines[1].starts_with("web") && lines[1].contains("4001"),
        "{lines:?}"
    );
    both_widths(c, "review-paths-folded").await;
    c.ui.click(&format!("[data-review-path=ssh] {LINE}")).await;
    c.ui.wait_true(
        "document.querySelector('[data-review-path=ssh]').open",
        "a click opens the line",
        SECS,
    )
    .await;
    assert_eq!(
        opened(c).await,
        [true, false],
        "a click opens that service only"
    );
    c.ui.wait_for("[data-review-path=ssh] [role=group]", SECS)
        .await;
    both_widths(c, "review-paths-one-open").await;
    c.ui.press(&format!("[data-review-path=ssh] {LINE}"), "Enter")
        .await;
    c.ui.wait_true(
        "!document.querySelector('[data-review-path=ssh]').open",
        "Enter on the line folds it again",
        SECS,
    )
    .await;
}

fn review_paths(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        one_service_has_its_diagram_without_a_fold(c).await;
        several_services_are_one_line_each(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn review_shows_one_diagram_or_one_line_per_changed_service() {
    run("review-paths", Options::default(), false, review_paths);
}
