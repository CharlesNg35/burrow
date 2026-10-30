use super::edits::{Flow, click_js, row_clean, row_error};
use super::world::{Options, free_range};
use super::{Ctx, SECS, applied, edit_mode, open, open_review, run, wait_checked};

fn weights(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/services/pool").await;
        edit_mode(c).await;
        let mode = "[data-schema='services/*/mode']";
        c.ui.wait_for(&format!("{mode} input[value=least_connections]"), SECS)
            .await;
        assert_eq!(
            c.ui.count("[data-weights]").await,
            0,
            "weights only show with balance"
        );
        click_js(c, &format!("{mode} input[value=balance]")).await;
        let home = "#f-services-pool-clients-weight-home";
        c.ui.wait_for(home, SECS).await;
        c.ui.wait_for("#f-services-pool-clients-weight-office", SECS)
            .await;
        c.ui.fill(home, "0").await;
        let err = row_error(c, "services/*/clients").await;
        assert!(err.contains("1 to 1000"), "{err}");
        c.ui.fill(home, "3").await;
        row_clean(c, "services/*/clients").await;
        c.ui.shot("balance-weights").await;
        wait_checked(c).await;
        c.ui.click("[data-testid=save]").await;
        open_review(c, "review-of-the-weights").await;
        applied(c, "weights-applied").await;
        let text = c.world.config();
        assert!(text.contains("mode: balance"), "{text}");
        assert!(text.contains("name: home"), "{text}");
        assert!(text.contains("weight: 3"), "{text}");
        c.ui.assert_no_errors();
    })
}

#[test]
fn balance_shows_a_weight_per_client_and_writes_it() {
    let opts = Options {
        extra_services: format!(
            "  pool:\n    clients: [home, office]\n    bind: 127.0.0.1:{}\n    target: 8080\n",
            free_range(1)
        ),
        ..Options::default()
    };
    run("weights", opts, false, weights);
}
