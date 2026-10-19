use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use super::world::Options;
use super::{Ctx, SECS, open, run};

type Flow<'a> = Pin<Box<dyn Future<Output = ()> + 'a>>;

async fn wait_checked(c: &Ctx) {
    c.ui.wait_for("[data-testid=pending-bar]", SECS).await;
    c.ui.wait_not_text("[data-testid=pending-bar]", "checking", SECS)
        .await;
}

async fn review_and_apply(c: &mut Ctx, what: &str) {
    wait_checked(c).await;
    c.ui.click("[data-testid=review-open]").await;
    c.ui.wait_for("[data-testid=apply]", SECS).await;
    c.ui.shot(&format!("review-{what}")).await;
    c.ui.click("[data-testid=apply]").await;
}

async fn storage_has(c: &Ctx, needle: &str) -> bool {
    let all: String =
        c.ui.eval("JSON.stringify([Object.entries(sessionStorage), Object.entries(localStorage)])")
            .await;
    all.contains(needle)
}

fn secret_once(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/clients").await;
        c.ui.click("[data-testid=new]").await;
        c.ui.fill("#new-name", "lab").await;
        c.ui.click("[data-testid=create]").await;
        c.ui.wait_for("[data-testid=detail][data-name=lab]", SECS)
            .await;
        c.ui.wait_for("[data-testid=secrets] [data-fp]", SECS).await;
        c.ui.shot("new-client-with-a-generated-secret").await;
        review_and_apply(c, "new-client").await;
        c.ui.wait_for("[data-testid=shown-once]", Duration::from_secs(20))
            .await;
        let secret =
            c.ui.text("[data-testid=secret-value] pre")
                .await
                .trim()
                .to_string();
        assert_eq!(secret.len(), 44, "a 256-bit base64 secret: {secret:?}");
        let config = c.ui.text("[data-testid=config-text] pre").await;
        assert!(config.contains("name: lab"), "{config}");
        assert!(config.contains(&secret), "{config}");
        c.ui.shot("secret-shown-once-with-client-yaml").await;
        c.ui.click("[data-testid=shown-once] [data-tab=commands]")
            .await;
        let commands = c.ui.text("[data-testid=snippet-commands]").await;
        assert!(
            commands.contains(&format!(
                "burrow init client --server {} --name lab",
                c.world.server_addr()
            )),
            "{commands}"
        );
        c.ui.shot("connection-commands").await;
        c.ui.click("[data-testid=shown-once] [data-tab=env]").await;
        c.ui.wait_text("[data-testid=snippet-env]", "LAB_SECRET", SECS)
            .await;
        c.ui.shot("environment-variable-form").await;
        assert!(
            !c.ui.enabled("[data-testid=close-shown-once]").await,
            "closing needs the saved box"
        );
        c.ui.click("[data-testid=saved]").await;
        c.ui.click("[data-testid=close-shown-once]").await;
        c.ui.wait_gone("[data-testid=shown-once]", SECS).await;

        c.world.write("lab.yaml", &config);
        c.world.start("lab", &["client", "-c", "lab.yaml"], &[]);
        c.world.wait_client("lab", true);
        c.ui.nav("/clients").await;
        c.ui.wait_text("[data-testid=entity-list] tr[data-row=lab]", "online", SECS)
            .await;
        c.ui.shot("lab-online-with-the-copied-config").await;
        c.ui.nav("/clients/lab").await;
        c.ui.wait_for("[data-testid=secrets] [data-fp]", SECS).await;
        let page: String = c.ui.eval("document.body.innerText").await;
        assert!(!page.contains(&secret), "the secret is not shown again");
        c.ui.shot("only-the-fingerprint-later").await;
        c.ui.click("[data-testid=connect-open]").await;
        c.ui.wait_for("[data-testid=connect] [data-testid=snippets]", SECS)
            .await;
        let later = c.ui.text("[data-testid=connect]").await;
        assert!(!later.contains(&secret), "{later}");
        c.ui.shot("connect-panel-without-the-secret").await;

        assert!(
            !storage_has(c, &secret).await,
            "the secret is not kept in browser storage"
        );
        let logs = c.world.logs();
        let lab_config = c.world.file("lab.yaml");
        let without_files =
            logs.replace(&std::fs::read_to_string(lab_config).unwrap_or_default(), "");
        assert!(
            !without_files
                .lines()
                .filter(|l| !l.starts_with("    secret:") && !l.contains("secret: "))
                .any(|l| l.contains(&secret)),
            "the secret is not in any log"
        );
        let text = c.world.config();
        assert!(
            text.contains("# the home box") && text.contains("# keep this comment"),
            "{text}"
        );
        c.ui.assert_no_errors();
    })
}

#[test]
fn e2_a_new_client_gets_its_secret_once_and_connects_with_the_shown_config() {
    run("e2-secret-once", Options::default(), false, secret_once);
}

fn rotation(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/clients/home").await;
        c.ui.wait_for(
            "[data-testid=secrets] [data-in-use=true]",
            Duration::from_secs(15),
        )
        .await;
        let old =
            c.ui.attr_of("[data-testid=secrets] [data-in-use=true]", "data-fp")
                .await;
        c.ui.shot("one-secret-in-use").await;
        c.ui.click("[data-testid=rotate]").await;
        c.ui.wait_for("[data-testid=secrets] li:nth-child(2)", SECS)
            .await;
        c.ui.shot("rotation-adds-a-second-secret").await;
        review_and_apply(c, "rotation").await;
        c.ui.wait_for("[data-testid=shown-once]", Duration::from_secs(20))
            .await;
        let new_secret =
            c.ui.text("[data-testid=secret-value] pre")
                .await
                .trim()
                .to_string();
        c.ui.shot("new-secret-shown-once").await;
        c.ui.click("[data-testid=saved]").await;
        c.ui.click("[data-testid=close-shown-once]").await;
        c.ui.wait_for("[data-testid=secrets] li:nth-child(2)", SECS)
            .await;
        let in_use =
            c.ui.attr_of("[data-testid=secrets] [data-in-use=true]", "data-fp")
                .await;
        assert_eq!(in_use, old, "the old secret is still the one in use");
        c.ui.shot("both-listed-old-in-use").await;

        c.ui.click(&format!(
            "[data-testid=secrets] [data-fp='{old}'] [data-testid=remove-secret]"
        ))
        .await;
        wait_checked(c).await;
        c.ui.click("[data-testid=review-open]").await;
        c.ui.wait_for("[data-testid=review] [data-confirm=home]", SECS)
            .await;
        c.ui.shot("removing-the-used-secret-needs-the-name").await;
        c.ui.click("[data-testid=review-drawer] button[aria-label=Close]")
            .await;
        c.ui.click("[data-testid=discard]").await;

        let yaml = format!(
            "server: {}\nname: home\nsecret: {new_secret}\nallow: [127.0.0.1/32]\n",
            c.world.server_addr()
        );
        c.world.write("home.yaml", &yaml);
        c.ui.wait_not_text(
            "[data-testid=secrets] [data-in-use=true]",
            &old,
            Duration::from_secs(20),
        )
        .await;
        c.ui.shot("new-secret-in-use-after-the-client-reloaded")
            .await;
        c.ui.click(&format!(
            "[data-testid=secrets] [data-fp='{old}'] [data-testid=remove-secret]"
        ))
        .await;
        wait_checked(c).await;
        c.ui.click("[data-testid=review-open]").await;
        c.ui.wait_for("[data-testid=apply]", SECS).await;
        assert_eq!(
            c.ui.count("[data-testid=review] [data-confirm]").await,
            0,
            "safe to remove now"
        );
        c.ui.shot("safe-to-remove").await;
        c.ui.click("[data-testid=apply]").await;
        c.ui.wait_for("[data-testid=outcome-applied]", Duration::from_secs(20))
            .await;
        c.world.wait_client("home", true);
        assert!(c.world.client_online("home"), "home stays online");
        c.ui.shot("old-secret-removed").await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn e6_rotating_a_secret_shows_which_one_is_in_use() {
    run("e6-rotation", Options::default(), false, rotation);
}

fn revoke_and_reconnect(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "viewer").await;
        c.ui.shot("viewer-connected").await;
        c.world.hand_edit(|t| t.replace("    admin: read\n", ""));
        c.ui.wait_for("[data-testid=revoked]", Duration::from_secs(8))
            .await;
        c.ui.shot("viewer-access-removed").await;

        let url = c.world.open_admin("ops");
        c.ui.goto("about:blank").await;
        c.ui.goto(&url).await;
        c.ui.wait_text("[data-testid=conn-pill]", "Connected", SECS)
            .await;
        c.ui.nav("/services/web").await;
        c.ui.fill("#f-services-web-target", "4100").await;
        wait_checked(c).await;
        c.ui.shot("ops-with-a-draft").await;
        c.world.stop("server");
        c.ui.wait_text(
            "[data-testid=conn-pill]",
            "Reconnecting",
            Duration::from_secs(15),
        )
        .await;
        c.ui.shot("server-gone-reconnecting").await;
        c.world
            .start("server", &["server", "-c", "server.yaml"], &[]);
        c.ui.wait_text(
            "[data-testid=conn-pill]",
            "Connected",
            Duration::from_secs(30),
        )
        .await;
        c.ui.wait_text("[data-testid=pending-bar]", "1 pending change", SECS)
            .await;
        let kept: String =
            c.ui.eval("document.getElementById('f-services-web-target').value")
                .await;
        assert_eq!(kept, "4100", "the draft survives the reconnect");
        c.ui.shot("reconnected-draft-kept").await;
        c.ui.take_errors();
    })
}

#[test]
fn e9_revoked_access_is_shown_and_a_restart_keeps_the_draft() {
    run(
        "e9-revoke-reconnect",
        Options::default(),
        false,
        revoke_and_reconnect,
    );
}
