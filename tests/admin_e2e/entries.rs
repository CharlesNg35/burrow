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

async fn shown(c: &Ctx, css: &str) -> bool {
    c.ui.eval(&format!(
        "(() => {{ const e = document.querySelector(\"{css}\"); return !!e && e.offsetParent !== null; }})()"
    ))
    .await
}

async fn phone_edit(c: &mut Ctx) {
    let sum = |i: usize| format!("[data-entry='{i}'] [data-entry-summary]");
    let weight = |i: usize| format!("#f-services-pool-clients-{i}-weight");
    c.ui.wait_for(&sum(2), SECS).await;
    let first = c.ui.text(&sum(0)).await;
    assert!(
        first.contains("home") && first.contains("→ 127.0.0.1:8080 · main · weight 3"),
        "{first}"
    );
    let second = c.ui.text(&sum(1)).await;
    assert!(second.contains("→ 10.0.0.13:8080 · own"), "{second}");
    for i in 0..3 {
        assert!(
            !shown(c, &weight(i)).await,
            "entries start folded on a phone"
        );
    }
    let overflow: bool =
        c.ui.eval("document.documentElement.scrollWidth > document.documentElement.clientWidth")
            .await;
    assert!(
        !overflow,
        "the entries fit a phone without sideways scrolling"
    );
    c.ui.shot("client-entries-edit-phone").await;
    c.ui.click("[data-entry='0'] [data-testid=entry-toggle]")
        .await;
    c.ui.wait_true(
        &format!(
            "document.querySelector('{}').offsetParent !== null",
            weight(0)
        ),
        "a tap opens the entry",
        SECS,
    )
    .await;
    c.ui.click("[data-entry='0'] [data-testid=step-up]").await;
    c.ui.wait_text(&sum(0), "weight 4", SECS).await;
    c.ui.shot("client-entry-open-phone").await;
    c.ui.click("[data-entry='1'] [data-testid=entry-toggle]")
        .await;
    c.ui.wait_true(
        &format!(
            "document.querySelector('{}').offsetParent !== null",
            weight(1)
        ),
        "the other entry opens",
        SECS,
    )
    .await;
    assert!(!shown(c, &weight(0)).await, "one entry open at a time");
    click_js(c, "[data-schema='services/*/mode'] input[value=failover]").await;
    c.ui.wait_for("[data-entry='0'] [data-entry-error]", SECS)
        .await;
    c.ui.wait_true(
        &format!(
            "document.querySelector('{}').offsetParent !== null",
            weight(0)
        ),
        "an entry with an error opens",
        SECS,
    )
    .await;
    assert!(!shown(c, &weight(1)).await, "still one open");
    c.ui.shot("client-entry-error-phone").await;
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
        let said =
            c.ui.wait_text(main, "main target · 127.0.0.1:8080", SECS)
                .await;
        assert!(said.contains("Change"), "{said}");
        c.ui.fill("#f-services-pool-target", "8081").await;
        c.ui.wait_text(main, "main target · 127.0.0.1:8081", SECS)
            .await;
        c.ui.fill("#f-services-pool-target", "8080").await;
        c.ui.wait_text(main, "main target · 127.0.0.1:8080", SECS)
            .await;
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
        c.ui.wait_text(main, "main target · 127.0.0.1:8080", SECS)
            .await;
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
            rows.contains("→ 10.0.0.13:8080") && rows.contains("→ 127.0.0.1:8080(main target)"),
            "one row per entry, each with its target: {rows}"
        );
        c.ui.shot("client-entries-health").await;
        c.ui.nav("/clients").await;
        c.ui.nav("/services/pool?tab=settings").await;
        c.ui.wait_for("[data-entry-line]", SECS).await;
        assert_eq!(c.ui.count("[data-entry-line]").await, 3);
        let lines = c.ui.text("[data-schema='services/*/clients']").await;
        assert!(lines.contains("→ 127.0.0.1:8080 (main target)"), "{lines}");
        assert!(lines.contains("→ 10.0.0.13:8080"), "{lines}");
        assert!(lines.contains("weight 3"), "{lines}");
        c.ui.shot("client-entries-view").await;
        c.ui.viewport(360, 800).await;
        c.ui.shot("client-entries-view-phone").await;
        edit_mode(c).await;
        phone_edit(c).await;
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
