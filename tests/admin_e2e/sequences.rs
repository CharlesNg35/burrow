use std::time::Duration;

use chromiumoxide::Page;

use super::edits::Flow;
use super::world::Options;
use super::{
    Ctx, SECS, applied, echo_server, edit_mode, open, open_review, round_trip, run, wait_checked,
};

async fn agree(c: &Ctx, step: &str, file_has: &[&str], web_port: u16) {
    let file = c.world.config();
    for want in file_has {
        assert!(file.contains(want), "{step}: the file has {want}:\n{file}");
    }
    let status = c.world.status().expect("burrow status answers");
    let bind = status["services"]
        .as_array()
        .and_then(|s| s.iter().find(|s| s["name"] == "web"))
        .map(|s| s["bind"].as_str().unwrap_or_default().to_string())
        .unwrap_or_default();
    assert!(
        bind.ends_with(&format!(":{web_port}")),
        "{step}: the running server binds web on {web_port}: {bind}"
    );
    let mut ok = false;
    for _ in 0..30 {
        if round_trip(web_port, 1000).await.is_ok() {
            ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(ok, "{step}: traffic flows through web on {web_port}");
}

async fn edit_target(c: &Ctx, value: &str) {
    c.ui.nav("/services/web?tab=settings").await;
    edit_mode(c).await;
    c.ui.fill("#f-services-web-target", value).await;
    wait_checked(c).await;
}

async fn add_to_changes(c: &Ctx) {
    c.ui.click("[data-testid=save]").await;
    c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
}

async fn changed_on_the_server(c: &Ctx) {
    let _ = c.ui.page.bring_to_front().await;
    c.ui.wait_for("[data-testid=conflict]", Duration::from_secs(15))
        .await;
    c.ui.click("[data-testid=conflict] button[aria-label=Close]")
        .await;
    c.ui.wait_gone("[data-testid=conflict]", SECS).await;
    c.ui.wait_for("[data-testid=banner-changed]", Duration::from_secs(15))
        .await;
}

fn discard_halfway(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        let _echo = echo_server(c.world.ports["echo"]).await;
        let web = c.world.ports["web"];
        let before = c.world.config();
        open(c, "ops").await;
        edit_target(c, "4555").await;
        c.ui.click("[data-testid=discard-edit]").await;
        c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
        c.ui.nav("/review").await;
        c.ui.wait_for("[data-testid=review-empty]", SECS).await;
        assert_eq!(c.ui.count("[data-testid=review-not-added]").await, 0);
        assert_eq!(c.world.config(), before, "a discarded edit leaves the file");
        agree(c, "after discard", &[], web).await;
        c.ui.shot("discarded-then-review").await;
        c.ui.assert_no_errors();
    })
}

async fn tab_eval(p: &Page, js: &str) -> serde_json::Value {
    match tokio::time::timeout(Duration::from_secs(5), p.evaluate(js)).await {
        Ok(Ok(v)) => v.into_value().unwrap_or_default(),
        _ => serde_json::Value::Null,
    }
}

async fn tab_wait(p: &Page, css: &str) {
    for _ in 0..100 {
        let js = format!("document.querySelector({css:?}) !== null");
        if tab_eval(p, &js).await == serde_json::Value::Bool(true) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let body = tab_eval(
        p,
        "document.body ? document.body.innerText.slice(0, 400) : ''",
    )
    .await;
    panic!("the second tab never showed {css}: {body}");
}

async fn tab_apply_target(p: &Page, service: &str, value: &str) {
    let _ = p.bring_to_front().await;
    let nav = format!(
        "(() => {{ history.pushState(null, '', location.pathname.match(/^\\/r-[0-9a-f]+/)[0] + '/services/{service}?tab=settings'); dispatchEvent(new PopStateEvent('popstate')); return true; }})()"
    );
    tab_eval(p, &nav).await;
    tab_wait(p, "[data-testid=edit]").await;
    tab_eval(p, "document.querySelector('[data-testid=edit]').click()").await;
    let id = format!("#f-services-{service}-target");
    tab_wait(p, &id).await;
    let fill = format!(
        "(() => {{ const i = document.querySelector({id:?}); const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set; set.call(i, {value:?}); i.dispatchEvent(new Event('input', {{ bubbles: true }})); i.dispatchEvent(new Event('blur')); return true; }})()"
    );
    tab_eval(p, &fill).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    tab_eval(p, "document.querySelector('[data-testid=save]').click()").await;
    tab_wait(p, "[data-testid=review-open]").await;
    tab_eval(
        p,
        "document.querySelector('[data-testid=review-open]').click()",
    )
    .await;
    tab_wait(p, "[data-testid=apply]:not([disabled])").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    tab_eval(p, "document.querySelector('[data-testid=apply]').click()").await;
    tokio::time::sleep(Duration::from_secs(3)).await;
}

fn two_tabs(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        let _echo = echo_server(c.world.ports["echo"]).await;
        let (web, echo) = (c.world.ports["web"], c.world.ports["echo"]);
        open(c, "ops").await;
        edit_target(c, &format!("127.0.0.1:{echo}")).await;
        add_to_changes(c).await;
        let url = c.world.open_admin("boss");
        let other = tokio::time::timeout(SECS * 2, c.ui.browser.new_page("about:blank"))
            .await
            .expect("a second tab in time")
            .expect("a second tab");
        let _ = tokio::time::timeout(SECS * 2, other.goto(url.as_str())).await;
        eprintln!("second tab at {url}");
        tab_wait(&other, "[data-testid=conn-pill]").await;
        eprintln!("second tab connected");
        tab_apply_target(&other, "ssh", "2222").await;
        assert!(
            c.world.config().contains("target: 2222"),
            "the other tab applied"
        );
        changed_on_the_server(c).await;
        c.ui.shot("first-tab-sees-the-other-apply").await;
        c.ui.click("[data-testid=review-open]").await;
        c.ui.wait_for("[data-testid=review-stale]", SECS).await;
        c.ui.click("[data-testid=review-stale] button").await;
        c.ui.wait_for("[data-testid=review]", SECS).await;
        wait_checked(c).await;
        open_review(c, "review-on-the-other-tabs-version").await;
        applied(c, "first-tab-applied-after").await;
        agree(
            c,
            "different entities",
            &["target: 2222", &format!("target: 127.0.0.1:{echo}")],
            web,
        )
        .await;

        c.ui.nav("/services/web?tab=settings").await;
        edit_mode(c).await;
        c.ui.fill("#f-services-web-target-host", "").await;
        c.ui.fill("#f-services-web-target", &echo.to_string()).await;
        wait_checked(c).await;
        add_to_changes(c).await;
        tab_apply_target(&other, "web", &format!("localhost:{echo}")).await;
        changed_on_the_server(c).await;
        c.ui.click("[data-testid=review-open]").await;
        c.ui.wait_for("[data-testid=review-stale]", SECS).await;
        c.ui.click("[data-testid=review-stale] button").await;
        c.ui.wait_for("[data-testid=review]", SECS).await;
        wait_checked(c).await;
        let review = c.ui.text("[data-testid=review]").await;
        assert!(
            review.contains(&format!("localhost:{echo}")) && review.contains(&echo.to_string()),
            "Review shows the other tab's value it replaces: {review}"
        );
        c.ui.shot("same-entity-review").await;
        open_review(c, "same-entity-review-open").await;
        applied(c, "same-entity-applied").await;
        agree(
            c,
            "same entity, last apply wins",
            &[&format!("target: {echo}")],
            web,
        )
        .await;
        let _ = other.close().await;
        c.ui.take_errors();
    })
}

fn restart_with_draft(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        let _echo = echo_server(c.world.ports["echo"]).await;
        let (web, echo) = (c.world.ports["web"], c.world.ports["echo"]);
        open(c, "ops").await;
        edit_target(c, &format!("127.0.0.1:{echo}")).await;
        add_to_changes(c).await;
        c.world.stop("server");
        c.ui.wait_text(
            "[data-testid=conn-pill]",
            "Reconnecting",
            Duration::from_secs(15),
        )
        .await;
        c.world
            .start("server", &["server", "-c", "server.yaml"], &[]);
        c.ui.wait_text(
            "[data-testid=conn-pill]",
            "Connected",
            Duration::from_secs(30),
        )
        .await;
        c.world.wait_client("home", true);
        c.ui.shot("draft-after-a-restart").await;
        open_review(c, "review-after-a-restart").await;
        applied(c, "applied-after-a-restart").await;
        agree(
            c,
            "after a restart",
            &[&format!("target: 127.0.0.1:{echo}")],
            web,
        )
        .await;
        c.ui.take_errors();
    })
}

fn disk_edit_with_draft(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        let _echo = echo_server(c.world.ports["echo"]).await;
        let (web, echo) = (c.world.ports["web"], c.world.ports["echo"]);
        open(c, "ops").await;
        edit_target(c, &format!("127.0.0.1:{echo}")).await;
        add_to_changes(c).await;
        c.world
            .hand_edit(|t| t.replacen("    target: 22\n", "    target: 2223\n", 1));
        c.ui.wait_for("[data-testid=conflict]", Duration::from_secs(15))
            .await;
        c.ui.shot("conflict-after-a-disk-edit").await;
        c.ui.click("[data-testid=conflict] button[aria-label=Close]")
            .await;
        c.ui.wait_gone("[data-testid=conflict]", SECS).await;
        c.ui.wait_for("[data-testid=banner-changed]", Duration::from_secs(15))
            .await;
        c.ui.click("[data-testid=review-open]").await;
        c.ui.wait_for("[data-testid=review-stale]", SECS).await;
        c.ui.click("[data-testid=review-stale] button").await;
        c.ui.wait_for("[data-testid=review]", SECS).await;
        wait_checked(c).await;
        open_review(c, "review-after-a-disk-edit").await;
        applied(c, "applied-after-a-disk-edit").await;
        agree(
            c,
            "after a disk edit",
            &["target: 2223", &format!("target: 127.0.0.1:{echo}")],
            web,
        )
        .await;
        c.ui.take_errors();
    })
}

fn revert_after_partial(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        let _echo = echo_server(c.world.ports["echo"]).await;
        let web = c.world.ports["web"];
        open(c, "ops").await;
        c.ui.nav("/services/web?tab=settings").await;
        edit_mode(c).await;
        c.ui.fill("#f-services-web-bind", "127.0.0.1:81").await;
        wait_checked(c).await;
        open_review(c, "bind-81-review").await;
        applied(c, "bind-81-applied").await;
        if c.world
            .status()
            .and_then(|s| s["services"].as_array().cloned())
            .unwrap_or_default()
            .iter()
            .any(|s| s["name"] == "web" && s["not_listening"].is_string())
        {
            c.ui.nav("/").await;
            let page =
                c.ui.wait_text(
                    "body",
                    "Service web is not listening",
                    Duration::from_secs(20),
                )
                .await;
            assert!(page.contains("a restart fails until it can"), "{page}");
            c.ui.shot("not-listening-in-needs-attention").await;
        }
        c.ui.nav("/activity").await;
        c.ui.wait_for("[data-testid=activity] [data-row]", SECS)
            .await;
        let revs =
            c.ui.all_attrs("[data-testid=activity] [data-row]", "data-row")
                .await;
        let older = revs.first().cloned().expect("the applied version");
        c.ui.click(&format!("[data-row='{older}'] [data-testid=list-row]"))
            .await;
        c.ui.wait_for("[data-testid=revert]", SECS).await;
        c.ui.click("[data-testid=revert]").await;
        c.ui.wait_for("[data-testid=review]", SECS).await;
        applied(c, "reverted-after-a-partial-apply").await;
        agree(
            c,
            "after the revert",
            &[&format!("bind: 127.0.0.1:{web}")],
            web,
        )
        .await;
        let still = c
            .world
            .status()
            .and_then(|s| s["services"].as_array().cloned())
            .unwrap_or_default();
        assert!(
            still.iter().all(|s| s["not_listening"].is_null()),
            "the not-listening notice goes once the service listens again: {still:?}"
        );
        c.ui.take_errors();
    })
}

#[test]
fn sequence_an_edit_discarded_halfway_leaves_review_file_and_traffic_alone() {
    run("seq-discard", Options::default(), false, discard_halfway);
}

#[test]
fn sequence_two_tabs_apply_different_and_same_entities() {
    run("seq-two-tabs", Options::default(), false, two_tabs);
}

#[test]
fn sequence_a_draft_survives_a_server_restart_and_applies() {
    run("seq-restart", Options::default(), false, restart_with_draft);
}

#[test]
fn sequence_a_disk_edit_while_a_draft_is_open_keeps_both() {
    run(
        "seq-disk-edit",
        Options::default(),
        false,
        disk_edit_with_draft,
    );
}

#[test]
fn sequence_a_revert_after_an_apply_that_could_not_listen_restores_traffic() {
    run(
        "seq-revert-partial",
        Options::default(),
        false,
        revert_after_partial,
    );
}
