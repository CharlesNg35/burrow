use std::time::Duration;

use super::edits::{Flow, click_js};
use super::world::{Options, secret};
use super::{Ctx, SECS, applied, edit_mode, open, open_review, run};

const LONG: &str = "hetzner-pve-stagging-backup-node-01";
const ROW: &str = "[data-entries] [data-entry='0']";
const IPV6: &str = "online · 2001:db8:85a3::8a2e:370:7334";

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

async fn options_text(c: &Ctx, list: &str) -> Vec<String> {
    c.ui.eval(&format!(
        "[...document.querySelectorAll('{list} [role=option]')].map(o => o.dataset.option + '|' + [...o.querySelectorAll('span')].map(s => s.textContent).filter(t => t).join('|'))"
    ))
    .await
}

async fn layout_problems(c: &Ctx, list: &str, what: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bad: Vec<String> = c
        .ui
        .eval(&format!(
            "[...document.querySelectorAll('{list} [role=option]')].filter(o => {{ const n = o.querySelector('.font-semibold'); return n.getClientRects().length !== 1 || n.scrollWidth > n.clientWidth + 1; }}).map(o => o.dataset.option)"
        ))
        .await;
    if !bad.is_empty() {
        out.push(format!("{what}: names wrapped or cut: {bad:?}"));
    }
    let on_screen: bool = c
        .ui
        .eval(&format!(
            "(() => {{ const p = document.querySelector('{list}').parentElement.getBoundingClientRect(); return p.left >= 0 && p.right <= document.documentElement.clientWidth; }})()"
        ))
        .await;
    if !on_screen {
        out.push(format!("{what}: the dropdown leaves the screen"));
    }
    let wide: bool =
        c.ui.eval("document.documentElement.scrollWidth > document.documentElement.clientWidth")
            .await;
    if wide {
        out.push(format!("{what}: the page scrolls sideways"));
    }
    out
}

async fn shot_ipv6(c: &mut Ctx, list: &str, step: &str) {
    let _: bool = c
        .ui
        .eval(&format!(
            "(() => {{ const f = document.querySelector('{list} [data-option=home] [data-facet]'); if (f) f.textContent = '{IPV6}'; return true; }})()"
        ))
        .await;
    c.ui.shot(step).await;
}

async fn open_row_picker(c: &Ctx) -> String {
    if c.ui
        .count(&format!(
            "{ROW} [data-testid=entry-toggle]:not([aria-expanded=true])"
        ))
        .await
        > 0
        && c.ui
            .eval::<bool>(&format!(
                "document.querySelector(\"{ROW} [data-testid=entry-toggle]\").offsetParent !== null"
            ))
            .await
    {
        c.ui.click(&format!("{ROW} [data-testid=entry-toggle]"))
            .await;
    }
    let input = format!("{ROW} input[role=combobox]");
    c.ui.click(&input).await;
    let list =
        c.ui.eval::<String>(&format!(
            "'#' + document.querySelector(\"{input}\").getAttribute('aria-controls')"
        ))
        .await;
    c.ui.wait_for(&format!("{list} [data-option=self]"), SECS)
        .await;
    settle().await;
    list
}

fn picker_layout(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/services/web").await;
        c.ui.wait_for("[role=tablist] [data-tab=settings]", SECS)
            .await;
        c.ui.click("[role=tablist] [data-tab=settings]").await;
        edit_mode(c).await;
        let list = open_row_picker(c).await;
        let rows = options_text(c, &list).await;
        c.ui.shot("client-picker").await;
        let mut problems = layout_problems(c, &list, "desktop").await;
        shot_ipv6(c, &list, "client-picker-ipv6").await;
        problems.extend(layout_problems(c, &list, "desktop with IPv6").await);
        let _: bool =
            c.ui.eval("(() => { document.activeElement.blur(); return true; })()")
                .await;
        c.ui.viewport(390, 860).await;
        settle().await;
        let list = open_row_picker(c).await;
        c.ui.shot("client-picker-phone").await;
        problems.extend(layout_problems(c, &list, "phone").await);
        shot_ipv6(c, &list, "client-picker-phone-ipv6").await;
        problems.extend(layout_problems(c, &list, "phone with IPv6").await);
        c.ui.viewport(1440, 900).await;
        assert_eq!(rows[0], "self|This server|self", "{rows:?}");
        assert!(
            rows.iter()
                .any(|r| r.starts_with("home|home|online · 127.0.0.1")),
            "{rows:?}"
        );
        assert!(
            rows.iter().any(|r| r == "office|office|offline"),
            "{rows:?}"
        );
        assert!(
            rows.iter().any(|r| r == &format!("{LONG}|{LONG}|offline")),
            "{rows:?}"
        );
        assert!(problems.is_empty(), "{problems:#?}");
        c.ui.assert_no_errors();
    })
}

#[test]
fn the_client_picker_shows_each_client_on_one_line_with_a_short_status() {
    run("client-picker", opts(), false, picker_layout);
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
        c.ui.click("#f-services--new-client").await;
        c.ui.wait_for("#f-services--new-client-list [role=option]", SECS)
            .await;
        settle().await;
        c.ui.shot("new-website-client-picker").await;
        c.ui.viewport(390, 860).await;
        settle().await;
        c.ui.shot("new-website-client-picker-phone").await;
        c.ui.viewport(1440, 900).await;
        c.ui.wait_for("#f-services--new-client-list [data-option=self]", SECS)
            .await;
        let label =
            c.ui.text("#f-services--new-client-list [data-option=self]")
                .await;
        assert!(label.contains("This server"), "{label}");
        c.ui.click("#f-services--new-client-list [data-option=self]")
            .await;
        c.ui.wait_for("[data-testid=new-website] [data-chip=self]", SECS)
            .await;
        c.ui.fill("#f-services--new-target", "8081").await;
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

fn far_opts() -> Options {
    Options {
        extra_services: format!(
            "  far:\n    clients: [{LONG}, etiolles-ai-pve]\n    target: 8080\n    visitors: [laptop]\n"
        ),
        ..opts()
    }
}

async fn tall_triggers(c: &Ctx) -> Vec<String> {
    c.ui.eval(
        "[...document.querySelectorAll('[data-entries] [data-entry] input[role=combobox]')].filter(i => i.offsetParent !== null).map(i => i.parentElement.getBoundingClientRect().height).filter(h => h > 44).map(String)",
    )
    .await
}

fn closed_pickers(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/services/far").await;
        c.ui.wait_for("[role=tablist] [data-tab=settings]", SECS)
            .await;
        c.ui.click("[role=tablist] [data-tab=settings]").await;
        edit_mode(c).await;
        let mut problems = Vec::new();
        for w in [1440, 1280, 768] {
            c.ui.viewport(w, 900).await;
            settle().await;
            let tall = tall_triggers(c).await;
            if !tall.is_empty() {
                problems.push(format!("{w} px: closed pickers {tall:?} px tall"));
            }
        }
        c.ui.viewport(1440, 900).await;
        settle().await;
        c.ui.shot("closed-pickers").await;
        let list = open_row_picker(c).await;
        let active: String = c
            .ui
            .eval(&format!(
                "document.getElementById(document.querySelector(\"{ROW} input[role=combobox]\").getAttribute('aria-activedescendant'))?.dataset.option ?? ''"
            ))
            .await;
        assert!(
            c.ui.count(&format!("{list} [role=option]")).await > 2,
            "the picker lists the clients"
        );
        c.ui.shot("picker-opens-at-the-selected-client").await;
        assert_eq!(active, LONG, "opening starts at the selected client");
        assert!(problems.is_empty(), "{problems:#?}");
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_closed_client_picker_is_one_line_and_opens_at_its_client() {
    run("closed-pickers", far_opts(), false, closed_pickers);
}
