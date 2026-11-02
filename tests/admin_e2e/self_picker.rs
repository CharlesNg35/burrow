use std::time::Duration;

use super::edits::{Flow, click_js};
use super::world::{Options, secret};
use super::{Ctx, SECS, applied, edit_mode, open, open_review, run};

const LONG: &str = "hetzner-pve-stagging-backup-node-01";

fn opts() -> Options {
    Options {
        extra_top: "web:\n  https: 127.0.0.1:HTTPS_PORT\n".into(),
        extra_clients: format!(
            "  {LONG}:\n    secret: {}\n  etiolles-ai-pve:\n    secret: {}\n",
            secret(),
            secret()
        ),
        ..Options::default()
    }
}

async fn settle() {
    tokio::time::sleep(Duration::from_millis(400)).await;
}

async fn add_name(c: &Ctx, name: &str) {
    c.ui.fill("#new-website-names", name).await;
    c.ui.press("#new-website-names", "Enter").await;
    c.ui.wait_for(&format!("[data-chip='{name}']"), SECS).await;
}

fn website_on_self(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/services").await;
        c.ui.click("[data-testid=new]").await;
        c.ui.click("[data-new=website]").await;
        c.ui.wait_for("[data-testid=new-website]", SECS).await;
        add_name(c, "own.example.com").await;
        c.ui.click("#new-website-client").await;
        c.ui.wait_for("#new-website-client-list [role=option]", SECS)
            .await;
        settle().await;
        c.ui.shot("new-website-client-picker").await;
        c.ui.viewport(390, 860).await;
        settle().await;
        c.ui.shot("new-website-client-picker-phone").await;
        c.ui.viewport(1440, 900).await;
        c.ui.wait_for("#new-website-client-list [data-option=self]", SECS)
            .await;
        let label =
            c.ui.text("#new-website-client-list [data-option=self]")
                .await;
        assert!(label.contains("This server"), "{label}");
        c.ui.click("#new-website-client-list [data-option=self]")
            .await;
        c.ui.wait_for("[data-testid=new-website] [data-chip=self]", SECS)
            .await;
        c.ui.fill("#new-website-target", "8081").await;
        click_js(c, "[data-testid=new-website-tls] input[value=passthrough]").await;
        let path = c.ui.text("[data-testid=new-website-path]").await;
        assert!(
            path.contains("This server"),
            "the path names this server: {path}"
        );
        c.ui.click("[data-testid=create]").await;
        c.ui.wait_for("[data-testid=entity-list] [data-row=own]", SECS)
            .await;
        open_review(c, "website-on-self-review").await;
        applied(c, "website-on-self-applied").await;
        let text = c.world.config();
        assert!(
            text.contains("  own:\n")
                && text.contains("client: self")
                && text.contains("own.example.com"),
            "{text}"
        );
        c.ui.nav("/visitors/laptop").await;
        c.ui.wait_for("[role=tablist] [data-tab=access]", SECS)
            .await;
        c.ui.click("[role=tablist] [data-tab=access]").await;
        edit_mode(c).await;
        c.ui.click("#f-visitors-laptop-reach-add").await;
        c.ui.wait_for(
            "#f-visitors-laptop-reach-add-list [data-option=self][aria-disabled=true]",
            SECS,
        )
        .await;
        settle().await;
        c.ui.shot("reach-picker-self-off").await;
        let why =
            c.ui.text("#f-visitors-laptop-reach-add-list [data-option=self]")
                .await;
        assert!(why.contains("May reach on this server"), "{why}");
        c.ui.click("#f-visitors-laptop-reach-add-list [data-option=self]")
            .await;
        settle().await;
        assert_eq!(
            c.ui.count("[data-schema='visitors/*/reach'] [data-key=self]")
                .await,
            0,
            "this server cannot be added before it may reach anything"
        );
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_new_website_runs_on_this_server() {
    run("website-on-self", opts(), false, website_on_self);
}
