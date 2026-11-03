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
        "[data-testid=server-update-tag]",
        &format!("{to} available"),
        Duration::from_secs(60),
    )
    .await;
    c.ui.wait_for("[data-testid=server-update]:not([disabled])", SECS)
        .await;
    assert_eq!(
        c.ui.attr_of("[data-testid=server-whats-new]", "href").await,
        format!("https://github.com/CharlesNg35/burrow/releases/tag/v{to}")
    );
    assert_eq!(
        c.ui.count("[data-testid=server-burrow] [data-testid=server-whats-new]")
            .await,
        1,
        "the release's page is one click away on the same line"
    );
    super::updates::every_width(
        c,
        &format!("{step}-offer"),
        &["[data-testid=server-burrow]"],
    )
    .await;
    c.ui.click("[data-testid=server-update]").await;
    c.ui.wait_for("[data-testid=server-update-dialog]", SECS)
        .await;
    super::updates::whats_new(c, "server-update-dialog", to, "What's new").await;
    let said = c.ui.text("[data-testid=server-update-dialog]").await;
    assert!(said.contains("pause for a few seconds"), "{said}");
    every_look(c, &format!("{step}-dialog")).await;
    c.ui.click("[data-testid=server-update-confirm]").await;
    c.ui.wait_text(
        "[data-testid=server-update-state]",
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
        c.ui.wait_for("[data-testid=overview-sub]", SECS).await;
        assert_eq!(
            c.ui.count("[data-testid=server-burrow]").await,
            0,
            "nothing newer yet: the header is as in 0.2.2"
        );
        every_look(c, "nothing-to-say").await;
        latest("9.0.0-test.2");
        ask(c, "9.0.0-test.2", "update").await;
        c.ui.wait_text(
            "[data-testid=server-update-state]",
            "updated 9.0.0-test.1 → 9.0.0-test.2",
            RESTART,
        )
        .await;
        c.ui.wait_text("[data-testid=toasts]", "updated", SECS)
            .await;
        let why =
            c.ui.attr_of("[data-testid=server-update-state]", "title")
                .await;
        assert!(why.contains("asked by ops"), "{why}");
        let sub = c.ui.text("[data-testid=overview-sub]").await;
        assert!(
            sub.starts_with("v9.0.0-test.2 · "),
            "the header and the update line read the same running version: {sub}"
        );
        every_look(c, "updated").await;
        latest("9.0.0-test.6");
        ask(c, "9.0.0-test.6", "health").await;
        c.ui.wait_text(
            "[data-testid=server-update-state]",
            "Update to 9.0.0-test.6 rolled back",
            RESTART,
        )
        .await;
        assert_eq!(
            c.ui.text("[data-testid=server-update-still]").await,
            "still on 9.0.0-test.2"
        );
        assert_eq!(
            c.ui.count("[data-testid=server-update]").await,
            0,
            "no Update on that line"
        );
        let item = "[data-item=server-update]";
        let text = c.ui.text(item).await;
        assert!(
            text.contains("didn't become healthy within a minute, so 9.0.0-test.2 was put back"),
            "{text}"
        );
        assert!(
            !text.contains("endpoint"),
            "the raw reason is folded: {text}"
        );
        c.ui.click(&format!("{item} summary")).await;
        assert!(c.ui.text(item).await.contains("local status endpoint"));
        c.ui.wait_text(
            "[data-testid=toasts]",
            "Update rolled back — see Needs attention",
            SECS,
        )
        .await;
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
