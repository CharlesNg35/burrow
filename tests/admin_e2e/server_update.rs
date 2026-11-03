use std::io::Write;
use std::time::Duration;

use super::world::Options;
use super::{Ctx, SECS, both_widths, run};

const LIVE: &str = "BURROW_E2E_LIVE_OPS";
const CTL: &str = "BURROW_E2E_LIVE_CTL";
const RESTART: Duration = Duration::from_secs(180);

async fn every_look(c: &mut Ctx, step: &str) {
    both_widths(c, step).await;
    c.ui.viewport(1280, 900).await;
    c.ui.shot(&format!("{step}-1280")).await;
    c.ui.viewport(390, 900).await;
    c.ui.shot(&format!("{step}-390")).await;
    c.ui.viewport(1440, 900).await;
}

fn latest(version: &str) {
    let ctl = std::env::var(CTL).expect("the release server's control file");
    let mut f = std::fs::OpenOptions::new().append(true).open(ctl).unwrap();
    writeln!(f, "latest {version}").unwrap();
}

async fn ask(c: &mut Ctx, to: &str, step: &str) {
    c.ui.wait_text(
        "[data-testid=server-burrow] [data-testid=client-update-tag]",
        &format!("{to} available"),
        Duration::from_secs(60),
    )
    .await;
    c.ui.wait_for("[data-testid=server-update]:not([disabled])", SECS)
        .await;
    every_look(c, &format!("{step}-offer")).await;
    c.ui.click("[data-testid=server-update]").await;
    c.ui.wait_for("[data-testid=server-update-dialog]", SECS)
        .await;
    let said = c.ui.text("[data-testid=server-update-dialog]").await;
    assert!(said.contains("pause for a few seconds"), "{said}");
    every_look(c, &format!("{step}-dialog")).await;
    c.ui.click("[data-testid=server-update-confirm]").await;
    c.ui.wait_text(
        "[data-testid=server-burrow] [data-testid=client-update-tag]",
        "updating, the server restarts",
        SECS,
    )
    .await;
    c.ui.shot(&format!("{step}-asked")).await;
    c.ui.wait_for("[data-testid=banner-reconnecting]", RESTART)
        .await;
    let banner = c.ui.text("[data-testid=banner-reconnecting]").await;
    assert!(banner.contains("restarts with the new release"), "{banner}");
    both_widths(c, &format!("{step}-restarting")).await;
}

fn flow(c: &mut Ctx) -> super::edits::Flow<'_> {
    Box::pin(async move {
        let ops = std::env::var(LIVE).unwrap();
        let file = c.world.file("live-ops.yaml").display().to_string();
        std::fs::copy(&ops, &file).unwrap();
        let url = c.world.open_admin_with(&file, "live", 0);
        c.ui.goto(&url).await;
        c.ui.wait_gone("[data-testid=loading]", Duration::from_secs(30))
            .await;
        c.ui.wait_for("[data-testid=server-burrow]", SECS).await;
        let remote = c.ui.text("[data-testid=server-update-remote]").await;
        assert_eq!(remote, "updates from burrow admin on");
        latest("9.0.0-test.2");
        ask(c, "9.0.0-test.2", "update").await;
        c.ui.wait_text("[data-testid=server-update-last]", "updated", RESTART)
            .await;
        c.ui.wait_text("[data-testid=toasts]", "updated", SECS)
            .await;
        let last = c.ui.text("[data-testid=server-update-last]").await;
        assert!(last.contains("9.0.0-test.1 → 9.0.0-test.2"), "{last}");
        assert!(last.contains("asked by ops"), "{last}");
        let version =
            c.ui.text("[data-testid=server-burrow] [data-testid=client-version]")
                .await;
        assert_eq!(version, "9.0.0-test.2");
        every_look(c, "updated").await;
        latest("9.0.0-test.6");
        ask(c, "9.0.0-test.6", "health").await;
        c.ui.wait_text("[data-testid=server-update-last]", "rolled back", RESTART)
            .await;
        let last = c.ui.text("[data-testid=server-update-last]").await;
        assert!(last.contains("rolled back: "), "{last}");
        every_look(c, "rolled-back").await;
    })
}

#[test]
fn the_server_updates_itself_from_its_page_and_the_page_follows_the_restart() {
    if std::env::var_os(LIVE).is_none() {
        println!(
            "skipping: set {LIVE} to a visitor config of a server installed with update.remote: true"
        );
        return;
    }
    run("server-update-live", Options::default(), true, flow);
}
