use super::edits::{Flow, click_js, row_clean};
use super::world::{Options, free_range};
use super::{Ctx, SECS, applied, edit_mode, open, open_review, run, wait_checked};

fn entries(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/services/pool").await;
        edit_mode(c).await;
        let field = "services/*/clients";
        let mode = "[data-schema='services/*/mode']";
        c.ui.wait_for(&format!("{mode} input[value=least_connections]"), SECS)
            .await;
        c.ui.wait_for("[data-entry='1']", SECS).await;
        assert_eq!(
            c.ui.count("[data-entry] [data-weight]:not(.hidden)").await,
            0,
            "weights only show with balance"
        );
        click_js(c, &format!("{mode} input[value=balance]")).await;
        let weight = "#f-services-pool-clients-0-weight";
        c.ui.wait_for(weight, SECS).await;
        c.ui.fill(weight, "0").await;
        let row = "[data-entry='0'] [data-entry-error]";
        let err = c.ui.wait_text(row, "weight", SECS).await;
        assert!(
            err.contains("1 and 1000"),
            "the row shows its own error: {err}"
        );
        c.ui.shot("client-entry-row-error").await;
        c.ui.fill(weight, "3").await;
        c.ui.wait_gone(row, SECS).await;
        row_clean(c, field).await;
        click_js(c, "[data-entry='1'] [data-testid=entry-again]").await;
        c.ui.wait_for("[data-entry='2']", SECS).await;
        c.ui.fill("#f-services-pool-clients-2-target", "10.0.0.13:8080")
            .await;
        click_js(c, "[data-entry='2'] [data-testid=entry-up]").await;
        let mut moved = String::new();
        for _ in 0..50 {
            moved =
                c.ui.eval("document.getElementById('f-services-pool-clients-1-target').value")
                    .await;
            if moved == "10.0.0.13:8080" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert_eq!(moved, "10.0.0.13:8080", "the new entry moved up one row");
        c.ui.shot("client-entries-edit").await;
        wait_checked(c).await;
        c.ui.click("[data-testid=save]").await;
        open_review(c, "review-of-the-client-entries").await;
        applied(c, "client-entries-applied").await;
        let text = c.world.config();
        assert!(text.contains("mode: balance"), "{text}");
        assert!(text.contains("{ name: home, weight: 3 }"), "{text}");
        assert!(text.contains("10.0.0.13:8080"), "{text}");
        c.ui.nav("/services/pool").await;
        c.ui.wait_for("[data-testid=health] [data-row]", SECS).await;
        let rows = c.ui.text("[data-testid=health]").await;
        assert!(
            rows.contains("→ 10.0.0.13:8080") && rows.contains("(service target)"),
            "one row per entry, each with its target: {rows}"
        );
        c.ui.shot("client-entries-health").await;
        c.ui.nav("/clients").await;
        c.ui.nav("/services/pool?tab=settings").await;
        c.ui.wait_for("[data-entry-line]", SECS).await;
        assert_eq!(c.ui.count("[data-entry-line]").await, 3);
        let lines = c.ui.text("[data-schema='services/*/clients']").await;
        assert!(lines.contains("(service target)"), "{lines}");
        assert!(lines.contains("→ 10.0.0.13:8080"), "{lines}");
        assert!(lines.contains("weight 3"), "{lines}");
        c.ui.shot("client-entries-view").await;
        c.ui.viewport(390, 844).await;
        c.ui.shot("client-entries-view-phone").await;
        edit_mode(c).await;
        c.ui.wait_for("[data-entry='2']", SECS).await;
        let overflow: bool = c
            .ui
            .eval("document.documentElement.scrollWidth > document.documentElement.clientWidth")
            .await;
        assert!(
            !overflow,
            "the entry rows fit a phone without sideways scrolling"
        );
        c.ui.shot("client-entries-edit-phone").await;
        c.ui.click("[data-testid=discard-edit]").await;
        c.ui.nav("/clients").await;
        c.ui.nav("/services/pool").await;
        c.ui.wait_for("[data-testid=health] [data-row]", SECS).await;
        c.ui.shot("client-entries-health-phone").await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn client_entries_take_a_target_and_a_weight_and_can_repeat_and_move() {
    let opts = Options {
        extra_services: format!(
            "  pool:\n    clients: [home, office]\n    bind: 127.0.0.1:{}\n    target: 8080\n",
            free_range(1)
        ),
        ..Options::default()
    };
    run("client-entries", opts, false, entries);
}
