use super::edits::Flow;
use super::world::{self, Options};
use super::{Ctx, SECS, both_widths, open, overview_tab, run};

const CLIENTS: usize = 50;
const SERVICES: usize = 30;

fn many() -> Options {
    let extra_clients: String = (3..=CLIENTS)
        .map(|i| format!("  z{i:02}:\n    secret: {}\n", world::secret()))
        .collect();
    let extra_services: String = (1..=SERVICES)
        .map(|i| {
            format!(
                "  s{i:02}:\n    client: home\n    target: {}\n    visitors: [laptop]\n",
                9000 + i
            )
        })
        .chain([
            "  away:\n    client: office\n    target: 7001\n    visitors: [laptop]\n".to_string(),
        ])
        .collect();
    Options {
        extra_clients,
        extra_services,
        ..Options::default()
    }
}

async fn rows(c: &Ctx, table: &str) -> Vec<String> {
    c.ui.all_attrs(&format!("[data-testid={table}] tbody tr"), "data-row")
        .await
}

async fn at_most_eight(c: &mut Ctx, tab: &str, table: &str, total: usize) -> Vec<String> {
    overview_tab(c, tab).await;
    c.ui.wait_for(&format!("[data-testid={table}]"), SECS).await;
    let shown = rows(c, table).await;
    assert_eq!(shown.len(), 8, "{tab}: at most 8 rows: {shown:?}");
    let all =
        c.ui.wait_text(
            "[data-testid=overview-tabs] [data-testid=view-all]",
            &total.to_string(),
            SECS,
        )
        .await;
    assert_eq!(all, format!("View all {total} →"));
    assert_eq!(
        c.ui.count("[data-testid=overview-tabs] [data-testid=overview-open]")
            .await,
        0,
        "one link to the list: the footer's View all"
    );
    shown
}

fn tables(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        let clients = at_most_eight(c, "clients", "overview-clients", CLIENTS).await;
        assert_eq!(
            &clients[..3],
            ["office", "home", "z03"],
            "an away client that serves something comes first, then the rest by name"
        );
        let serves =
            c.ui.text("[data-testid=overview-clients] tr[data-row=home] [data-names]")
                .await;
        assert!(
            serves.starts_with("web, ssh, s01") && serves.contains("+29 more"),
            "the services column shows 3 names then how many more: {serves}"
        );
        assert_eq!(
            c.ui.count("[data-testid=overview-clients] [data-names] a, [data-testid=overview-clients] [data-names] button").await,
            0,
            "+N more is text, not a link"
        );
        both_widths(c, "overview-50-clients").await;
        c.ui.viewport(420, 900).await;
        assert_eq!(
            c.ui.count("[data-testid=mobile-clients] [data-row]").await,
            8,
            "the phone cards keep the same 8"
        );
        c.ui.viewport(1440, 900).await;
        c.ui.click("[data-testid=overview-tabs] [data-testid=view-all]")
            .await;
        c.ui.wait_for("[data-testid=entity-list] [data-row=z50]", SECS)
            .await;
        let home = c.ui.text("[data-testid=entity-list] [data-row=home]").await;
        assert!(
            !home.contains("s01"),
            "the sidebar shows no service names: {home}"
        );
        assert!(
            home.contains("127.0.0.1"),
            "the sidebar shows the IP: {home}"
        );
        c.ui.fill("#list-search", "s17").await;
        c.ui.wait_gone("[data-testid=entity-list] [data-row=z50]", SECS)
            .await;
        c.ui.wait_for("[data-testid=entity-list] [data-row=home]", SECS)
            .await;
        c.ui.shot("clients-sidebar-search-by-service").await;
        c.ui.fill("#list-search", "").await;
        c.ui.nav("/").await;
        let services = at_most_eight(c, "services", "overview-services", SERVICES + 3).await;
        assert_eq!(
            services[0], "away",
            "a service that is down comes first: {services:?}"
        );
        both_widths(c, "overview-32-services").await;
        c.ui.dark(true).await;
        overview_tab(c, "clients").await;
        both_widths(c, "overview-50-clients-dark").await;
        c.ui.dark(false).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn overview_tabs_show_8_rows_problems_first_and_a_link_to_all() {
    run("overview-tables", many(), false, tables);
}

const ROW_OF_FOCUS: &str = "(document.activeElement && document.activeElement.closest('[data-row]') || {}).dataset?.row || ''";

async fn ticks(c: &Ctx, css: &str) {
    let before = c.ui.text(css).await;
    c.ui.wait_not_text(css, &before, std::time::Duration::from_secs(10))
        .await;
}

fn live(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/clients/home").await;
        c.ui.wait_for(
            "[data-testid=list] [data-row=home][data-selected=true]",
            SECS,
        )
        .await;
        let bottom: f64 = c
            .ui
            .eval("(() => { const l = document.querySelector('[data-testid=list]'); l.scrollTop = l.scrollHeight; return l.scrollTop; })()")
            .await;
        assert!(bottom > 0.0, "the client list scrolls");
        ticks(c, "[data-testid=list] [data-row=home]").await;
        let kept: f64 =
            c.ui.eval("document.querySelector('[data-testid=list]').scrollTop")
                .await;
        assert!(
            (kept - bottom).abs() < 1.0,
            "a status tick leaves the list where the user scrolled it: {kept} vs {bottom}"
        );
        let _: bool = c
            .ui
            .eval("(document.querySelector('[data-testid=list] [data-row=z10] [data-testid=list-row]').focus(), true)")
            .await;
        ticks(c, "[data-testid=list] [data-row=home]").await;
        let row: String = c.ui.eval(ROW_OF_FOCUS).await;
        assert_eq!(row, "z10", "focus stays on the sidebar row through a tick");
        c.ui.nav("/").await;
        overview_tab(c, "services").await;
        c.ui.wait_for("[data-testid=overview-services] tbody tr", SECS)
            .await;
        let first: String = c
            .ui
            .eval("(() => { const a = document.querySelector('[data-testid=overview-services] tbody th a'); a.focus(); return a.closest('[data-row]').dataset.row; })()")
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
        let row: String = c.ui.eval(ROW_OF_FOCUS).await;
        assert_eq!(
            row, first,
            "focus stays on the Overview row through status ticks"
        );
        c.ui.assert_no_errors();
    })
}

#[test]
fn status_ticks_keep_the_list_scroll_and_the_focused_row() {
    run("live-lists", many(), false, live);
}
