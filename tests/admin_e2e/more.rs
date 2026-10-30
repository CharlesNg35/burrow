use std::time::Duration;

use super::edits::Flow;
use super::world::Options;
use super::{Ctx, SECS, edit_mode, open, run, wait_checked};

const TRAFFIC: &str = "[data-group=traffic] [data-testid=more-settings]";
const ACCESS: &str = "[data-group=access] [data-testid=more-settings]";

async fn expanded(c: &Ctx, css: &str) -> bool {
    c.ui.wait_for(css, SECS).await;
    c.ui.attr_of(css, "aria-expanded").await == "true"
}

async fn wait_expanded(c: &Ctx, css: &str, want: bool) {
    let started = std::time::Instant::now();
    while expanded(c, css).await != want {
        assert!(
            started.elapsed() < SECS,
            "{css} did not become expanded={want}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn region_hidden(c: &Ctx, css: &str) -> bool {
    c.ui.eval(&format!(
        "(() => {{ const b = document.querySelector({}); const r = document.getElementById(b.getAttribute('aria-controls')); return r.hasAttribute('hidden'); }})()",
        serde_json::to_string(css).expect("json")
    ))
    .await
}

async fn key_on(c: &Ctx, css: &str, key: &str) {
    let _: bool =
        c.ui.eval(&format!(
            "(() => {{ document.querySelector({}).focus(); return true; }})()",
            serde_json::to_string(css).expect("json")
        ))
        .await;
    c.ui.press(css, key).await;
}

async fn settings_tab(c: &Ctx, path: &str) {
    let tab = if path.starts_with("/visitors/") {
        "access"
    } else {
        "settings"
    };
    c.ui.nav(path).await;
    c.ui.wait_for("[role=tablist]", SECS).await;
    c.ui.click(&format!("[role=tablist] [data-tab={tab}]"))
        .await;
    c.ui.wait_for("[data-form-tab]", SECS).await;
}

async fn reload(c: &Ctx, path: &str) {
    let here: String = c.ui.eval("location.href").await;
    c.ui.goto(&here).await;
    c.ui.wait_gone("[data-testid=loading]", Duration::from_secs(20))
        .await;
    settings_tab(c, path).await;
}

async fn summaries(c: &Ctx) -> Vec<String> {
    c.ui.eval("Array.from(document.querySelectorAll('[data-form-tab] [data-testid=more-settings]')).map(b => b.textContent + '|' + b.getAttribute('aria-expanded'))")
        .await
}

async fn collapsed_and_remembered(c: &mut Ctx) {
    settings_tab(c, "/services/web").await;
    assert!(!expanded(c, ACCESS).await, "Who may connect starts closed");
    assert!(region_hidden(c, ACCESS).await);
    let text = c.ui.text(ACCESS).await;
    assert_eq!(text, "More settings · 3 at their defaults");
    assert_eq!(
        c.ui.count("[data-group=access] [data-schema='services/*/bandwidth']")
            .await,
        1,
        "a closed section keeps its fields in the page"
    );
    c.ui.shot("service-closed").await;
    c.ui.click(ACCESS).await;
    wait_expanded(c, ACCESS, true).await;
    assert!(!region_hidden(c, ACCESS).await);
    c.ui.shot("service-access-open").await;
    c.ui.nav("/services/ssh").await;
    settings_tab(c, "/services/web").await;
    assert!(expanded(c, ACCESS).await, "the choice survives navigation");
    reload(c, "/services/web").await;
    assert!(expanded(c, ACCESS).await, "the choice survives a reload");
    key_on(c, ACCESS, "Enter").await;
    wait_expanded(c, ACCESS, false).await;
    key_on(c, ACCESS, " ").await;
    wait_expanded(c, ACCESS, true).await;
    key_on(c, ACCESS, "Enter").await;
    wait_expanded(c, ACCESS, false).await;
    reload(c, "/services/web").await;
    assert!(!expanded(c, ACCESS).await, "closing is remembered too");
}

async fn set_fields_open(c: &mut Ctx) {
    settings_tab(c, "/visitors/ops").await;
    let admin = "[data-group=admin] [data-testid=more-settings]";
    assert!(expanded(c, admin).await, "a set Admin only from shows");
    assert_eq!(c.ui.text(admin).await, "More settings · 1 set");
    c.ui.shot("visitor-set-open").await;
    assert_eq!(
        c.ui.count("[data-group=traffic] [data-testid=more-settings]")
            .await,
        0,
        "a section whose only field is advanced shows it directly"
    );
    c.ui.wait_for(
        "[data-group=traffic] [data-schema='visitors/*/max_connections']",
        SECS,
    )
    .await;
    settings_tab(c, "/clients/office").await;
    assert_eq!(
        c.ui.count("[data-form-tab] [data-testid=more-settings]")
            .await,
        0,
        "one advanced field is not hidden behind More settings"
    );
    c.ui.wait_for(
        "[data-form-tab] [data-schema='clients/*/max_connections']",
        SECS,
    )
    .await;
    c.ui.shot("client-single-advanced").await;
    settings_tab(c, "/services/web").await;
    let view = summaries(c).await;
    edit_mode(c).await;
    assert_eq!(
        view,
        summaries(c).await,
        "view and edit show the same sections"
    );
    c.ui.shot("service-edit").await;
}

async fn error_opens(c: &mut Ctx) {
    assert_eq!(
        c.ui.count(TRAFFIC).await,
        0,
        "PROXY protocol, the only Traffic field, shows directly"
    );
    c.ui.click(ACCESS).await;
    wait_expanded(c, ACCESS, true).await;
    c.ui.fill("#f-services-web-max-connections", "9").await;
    wait_checked(c).await;
    c.ui.wait_text(ACCESS, "1 set", SECS).await;
    c.ui.click(ACCESS).await;
    wait_expanded(c, ACCESS, false).await;
    c.ui.choose("f-services-web-protocol", "udp").await;
    wait_checked(c).await;
    wait_expanded(c, ACCESS, true).await;
    c.ui.shot("service-error-opens").await;
    c.ui.click(ACCESS).await;
    wait_expanded(c, ACCESS, false).await;
    c.ui.click("[data-testid=form-problems] button").await;
    wait_expanded(c, ACCESS, true).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let focused: String = c
        .ui
        .eval("document.activeElement ? (document.activeElement.closest('[data-schema]') || {}).getAttribute?.('data-schema') || '' : ''")
        .await;
    assert_eq!(
        focused, "services/*/max_connections",
        "the link lands on the field"
    );
    c.ui.click(ACCESS).await;
    wait_expanded(c, ACCESS, false).await;
    c.ui.click("[data-testid=discard-edit]").await;
    c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
}

async fn found_opens(c: &mut Ctx) {
    settings_tab(c, "/services/web").await;
    assert!(!expanded(c, ACCESS).await, "Access starts closed");
    let found: bool = c
        .ui
        .eval("(() => { const r = document.querySelector('[data-group=access] [data-testid=more-region]'); return r.getAttribute('hidden') === 'until-found'; })()")
        .await;
    assert!(found, "the browser's find can open the section");
    let _: bool = c
        .ui
        .eval("(() => { document.querySelector('[data-group=access] [data-testid=more-region]').dispatchEvent(new Event('beforematch')); return true; })()")
        .await;
    wait_expanded(c, ACCESS, true).await;
    c.ui.click(ACCESS).await;
    wait_expanded(c, ACCESS, false).await;
    c.ui.nav("/settings?tab=connections").await;
    c.ui.wait_for("[data-testid=settings] [data-card=limits]", SECS)
        .await;
    let limits = "[data-card=limits] [data-testid=more-settings]";
    assert!(!expanded(c, limits).await, "Limits starts closed");
    c.ui.shot("settings-closed").await;
    c.ui.fill("#list-search", "handshake").await;
    wait_expanded(c, limits, true).await;
    assert_eq!(
        c.ui.count("[data-card='network.pool']").await,
        0,
        "only the matching section shows"
    );
    c.ui.shot("settings-search-opens").await;
    c.ui.fill("#list-search", "").await;
    wait_expanded(c, limits, false).await;
}

fn more(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        collapsed_and_remembered(c).await;
        set_fields_open(c).await;
        error_opens(c).await;
        found_opens(c).await;
        c.ui.dark(true).await;
        settings_tab(c, "/services/web").await;
        c.ui.click(ACCESS).await;
        wait_expanded(c, ACCESS, true).await;
        c.ui.shot("service-dark").await;
        c.ui.viewport(420, 900).await;
        c.ui.shot("service-dark-phone").await;
        c.ui.dark(false).await;
        c.ui.shot("service-phone").await;
        c.ui.viewport(1440, 900).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn more_settings_hide_advanced_fields_until_set_found_or_opened() {
    run("more-settings", Options::default(), false, more);
}
