use std::time::Duration;

use super::edits::Flow;
use super::world::Options;
use super::{Ctx, SECS, apply_now, both_widths, edit_mode, open, review_now, run, wait_checked};

const PILL: &str = "[data-testid=conn-pill]";
const BACK: Duration = Duration::from_secs(30);
const ACTIVITY: &str = "(() => document.querySelector('[data-testid=activity]').innerText)()";

async fn server_away(c: &mut Ctx) {
    c.world.stop("server");
    c.ui.wait_text(PILL, "Reconnecting", Duration::from_secs(15))
        .await;
}

async fn server_back(c: &mut Ctx) {
    c.world
        .start("server", &["server", "-c", "server.yaml"], &[]);
    c.ui.wait_text(PILL, "Connected", BACK).await;
}

fn retarget(c: &Ctx, to: u16) {
    c.world.hand_edit(|t| {
        let Some(line) = t.lines().find(|l| l.trim_start().starts_with("web: {")) else {
            return t.to_string();
        };
        let Some(at) = line.find("target: ").map(|a| a + "target: ".len()) else {
            return t.to_string();
        };
        let digits = line[at..].chars().take_while(char::is_ascii_digit).count();
        let new = format!("{}{to}{}", &line[..at], &line[at + digits..]);
        t.replacen(line, &new, 1)
    });
}

async fn empty_activity_follows_its_list_on_a_phone(c: &mut Ctx) {
    c.ui.viewport(420, 900).await;
    c.ui.nav("/activity").await;
    c.ui.wait_for("[data-testid=activity-empty]", SECS).await;
    let seen: Vec<f64> = c
        .ui
        .eval("(() => [document.querySelector('[data-testid=activity]').getBoundingClientRect().bottom, document.querySelector('[data-testid=detail-pane]').getBoundingClientRect().top])()")
        .await;
    assert!(
        seen[1] - seen[0] <= 1.0,
        "on a phone the empty Activity message follows the list without a gap [list bottom, message pane top]: {seen:?}"
    );
    c.ui.view_shot("activity-empty-phone").await;
    c.ui.viewport(1440, 900).await;
}

async fn a_detail_page_shows_what_changed_while_away(c: &mut Ctx) {
    c.ui.nav("/services/web").await;
    c.ui.wait_for("[data-testid=detail][data-name=web]", SECS)
        .await;
    server_away(c).await;
    retarget(c, 4321);
    server_back(c).await;
    c.ui.wait_text("[data-testid=detail]", "127.0.0.1:4321", BACK)
        .await;
    both_widths(c, "reconnect-detail").await;
}

async fn activity_shows_what_changed_while_away(c: &mut Ctx) {
    c.ui.nav("/services/web").await;
    edit_mode(c).await;
    c.ui.fill("#f-services-web-target", "4400").await;
    wait_checked(c).await;
    review_now(c).await;
    apply_now(c, "Applied: now the version of").await;
    c.ui.nav("/activity").await;
    c.ui.wait_for("[data-testid=activity] [data-testid=list-row]", SECS)
        .await;
    let before: String = c.ui.eval(ACTIVITY).await;
    server_away(c).await;
    retarget(c, 4322);
    server_back(c).await;
    c.ui.wait_true(
        &format!("{ACTIVITY} !== {}", serde_json::Value::from(before.clone())),
        "Activity shows the version written while the tab was away",
        BACK,
    )
    .await;
    both_widths(c, "reconnect-activity").await;
}

async fn the_file_loads_once_the_connection_is_back(c: &mut Ctx) {
    server_away(c).await;
    c.ui.nav("/settings?tab=file").await;
    c.ui.wait_for(
        "[data-testid=settings-tabs] [data-tab=file][aria-selected=true]",
        SECS,
    )
    .await;
    server_back(c).await;
    c.ui.wait_text("[data-testid=file-text] pre", "services:", BACK)
        .await;
    both_widths(c, "reconnect-file").await;
}

async fn the_connect_tab_loads_once_the_connection_is_back(c: &mut Ctx) {
    server_away(c).await;
    c.ui.nav("/clients/home?tab=connect").await;
    c.ui.wait_for("[data-testid=detail][data-name=home]", SECS)
        .await;
    server_back(c).await;
    c.ui.wait_for("[data-testid=connect] [data-testid=snippets]", BACK)
        .await;
    both_widths(c, "reconnect-connect").await;
}

fn reconnects(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        empty_activity_follows_its_list_on_a_phone(c).await;
        a_detail_page_shows_what_changed_while_away(c).await;
        activity_shows_what_changed_while_away(c).await;
        the_file_loads_once_the_connection_is_back(c).await;
        the_connect_tab_loads_once_the_connection_is_back(c).await;
        c.ui.take_errors();
    })
}

#[test]
fn screens_show_fresh_data_after_the_connection_comes_back() {
    run("reconnects", Options::default(), false, reconnects);
}
