use super::edits::{Flow, click_js, row_clean, row_error};
use super::world::{Options, free_range};
use super::{Ctx, SECS, applied, edit_mode, open, open_review, run, wait_checked};

async fn type_then(c: &Ctx, css: &str, text: &str, key: &str) {
    let _: bool = c
        .ui
        .eval(&format!(
            "(() => {{ const e = document.querySelector(\"{css}\"); e.focus(); e.value = '{text}'; e.dispatchEvent(new Event('input', {{ bubbles: true }})); return true; }})()"
        ))
        .await;
    c.ui.press(css, key).await;
}

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
        click_js(c, &format!("{mode} input[value=failover]")).await;
        c.ui.wait_for("[data-stray-weights]", SECS).await;
        let err = c.ui.wait_text(row, "Balance", SECS).await;
        assert!(
            err.starts_with("A weight") && !err.contains("mode: balance"),
            "plain words: {err}"
        );
        assert_eq!(
            c.ui.attr_of(weight, "aria-invalid").await,
            "true",
            "the weight that blocks saving is outlined"
        );
        assert!(
            c.ui.count("[data-entry] [data-weight]:not(.hidden)").await > 0,
            "a weight that blocks saving stays in sight"
        );
        wait_checked(c).await;
        assert!(!c.ui.enabled("[data-testid=save]").await);
        c.ui.shot("weights-after-mode-change").await;
        click_js(c, "[data-testid=clear-weights]").await;
        c.ui.wait_gone("[data-stray-weights]", SECS).await;
        c.ui.wait_gone(row, SECS).await;
        assert_eq!(
            c.ui.count("[data-entry] [data-weight]:not(.hidden)").await,
            0,
            "with no weights and no Balance the column goes"
        );
        click_js(c, &format!("{mode} input[value=balance]")).await;
        c.ui.wait_for(weight, SECS).await;
        c.ui.fill(weight, "3").await;
        row_clean(c, field).await;
        assert_eq!(
            c.ui.count("[data-entry] [data-own-target]").await,
            0,
            "a row has no target box until Change"
        );
        let main = "[data-entry='0'] [data-main-target]";
        let said = c.ui.wait_text(main, "main target · 8080", SECS).await;
        assert!(said.contains("Change"), "{said}");
        c.ui.fill("#f-services-pool-target", "8081").await;
        c.ui.wait_text(main, "main target · 8081", SECS).await;
        c.ui.fill("#f-services-pool-target", "8080").await;
        c.ui.wait_text(main, "main target · 8080", SECS).await;
        c.ui.fill("#f-services-pool-target", "").await;
        c.ui.wait_text(main, "main target · not set", SECS).await;
        let err = row_error(c, "services/*/target").await;
        assert!(!err.is_empty(), "the main target is required");
        wait_checked(c).await;
        let listed =
            c.ui.wait_text("[data-testid=form-problems]", "1 problem", SECS)
                .await;
        assert!(
            !listed.contains("missing field") && !listed.contains("2 problems"),
            "the empty main target is one problem in plain words: {listed}"
        );
        assert!(
            listed.contains("Target on the client: required"),
            "the label is not repeated: {listed}"
        );
        c.ui.wait_text("[data-testid=list] [data-row=pool]", "→ not set", SECS)
            .await;
        let bar = c.ui.text("[data-testid=save-bar]").await;
        assert!(!bar.contains("missing field"), "{bar}");
        assert!(
            !c.ui.enabled("[data-testid=save]").await,
            "no main target, no save"
        );
        click_js(c, "[data-entry='0'] [data-testid=entry-change]").await;
        c.ui.wait_for("#f-services-pool-clients-0-target", SECS)
            .await;
        c.ui.shot("client-entries-empty-main-target").await;
        c.ui.press("#f-services-pool-clients-0-target", "Escape")
            .await;
        c.ui.wait_for(main, SECS).await;
        c.ui.fill("#f-services-pool-target", "8080").await;
        c.ui.wait_text(main, "main target · 8080", SECS).await;
        click_js(c, "[data-entry='0'] [data-testid=entry-change]").await;
        c.ui.wait_true(
            "document.activeElement && document.activeElement.id === 'f-services-pool-clients-0-target'",
            "Change puts the cursor in the row's own target",
            SECS,
        )
        .await;
        let input0 = "#f-services-pool-clients-0-target";
        let own0 = "[data-entry='0'] [data-own-text]";
        type_then(c, input0, "10.0.0.12:3000", "Enter").await;
        let said = c.ui.wait_text(own0, "10.0.0.12:3000", SECS).await;
        assert!(said.contains("own") && said.contains("Change"), "{said}");
        assert_eq!(c.ui.count(input0).await, 0, "Enter closes the input");
        c.ui.shot("client-entry-own-target").await;
        click_js(c, "[data-entry='0'] [data-testid=entry-change]").await;
        c.ui.wait_for(input0, SECS).await;
        type_then(c, input0, "10.0.0.99:1", "Escape").await;
        c.ui.wait_text(own0, "10.0.0.12:3000", SECS).await;
        click_js(c, "[data-entry='0'] [data-testid=entry-change]").await;
        c.ui.wait_for(input0, SECS).await;
        c.ui.click("[data-entry='0'] [data-testid=own-target-clear]")
            .await;
        c.ui.wait_for(main, SECS).await;
        click_js(c, "[data-entry='0'] [data-testid=entry-change]").await;
        type_then(c, input0, "10.0.0.12:3000", "Enter").await;
        c.ui.wait_for(own0, SECS).await;
        click_js(c, "[data-entry='0'] [data-testid=entry-main]").await;
        c.ui.wait_for(main, SECS).await;
        click_js(c, "[data-entry='1'] [data-testid=entry-again]").await;
        let input2 = "#f-services-pool-clients-2-target";
        c.ui.wait_true(
            "document.activeElement && document.activeElement.id === 'f-services-pool-clients-2-target'",
            "the added entry opens its target with the cursor in it",
            SECS,
        )
        .await;
        c.ui.fill(input2, "10.0.0.13:8080").await;
        c.ui.wait_for("[data-entry='2'] [data-own-text]", SECS)
            .await;
        click_js(c, "[data-entry='2'] [data-testid=entry-up]").await;
        c.ui.wait_text("[data-entry='1'] [data-own-text]", "10.0.0.13:8080", SECS)
            .await;
        c.ui.wait_for("[data-entry='2'] [data-main-target]", SECS)
            .await;
        click_js(c, "[data-entry='2'] [data-testid=entry-change]").await;
        c.ui.wait_for("[data-entry='2'] [data-own-target]", SECS)
            .await;
        c.ui.shot("client-entries-edit").await;
        wait_checked(c).await;
        c.ui.click("[data-testid=save]").await;
        open_review(c, "review-of-the-client-entries").await;
        applied(c, "client-entries-applied").await;
        let text = c.world.config();
        assert!(text.contains("mode: balance"), "{text}");
        assert!(text.contains("{ name: home, weight: 3 }"), "{text}");
        assert!(text.contains("10.0.0.13:8080"), "{text}");
        assert!(
            !text.contains("10.0.0.12:3000"),
            "× went back to the main target: {text}"
        );
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
        c.ui.viewport(360, 800).await;
        c.ui.shot("client-entries-view-phone").await;
        edit_mode(c).await;
        c.ui.wait_for("[data-entry='2'] [data-main-target]", SECS)
            .await;
        click_js(c, "[data-entry='2'] [data-testid=entry-change]").await;
        c.ui.wait_for("[data-entry='2'] [data-own-target]", SECS)
            .await;
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
