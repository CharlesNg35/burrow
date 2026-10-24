use std::time::Duration;

use super::edits::{Flow, clear_of_bar, click_js};
use super::world::Options;
use super::{Ctx, SECS, edit_mode, open, run, wait_checked};

fn edit_state(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/visitors/laptop").await;
        c.world.hand_edit(|t| {
            t.replace(
                "  plain:\n",
                "  plain:\n    reach: { home: [127.0.0.1/32] }\n",
            )
        });
        c.ui.wait_text(
            "[data-testid=entity-list] [data-row=plain]",
            "reach: home",
            SECS,
        )
        .await;
        assert_eq!(
            c.ui.count("[data-testid=banner-changed]").await,
            0,
            "without a draft a hand edit just shows up, with no banner to act on"
        );
        c.ui.click("[role=tablist] [data-tab=access]").await;
        c.ui.wait_for("[data-schema='visitors/*/reach']", SECS)
            .await;
        assert_eq!(
            c.ui.count("[data-editor]").await,
            0,
            "view mode shows plain values"
        );
        assert_eq!(
            c.ui.count("[role=tablist] [data-tab=settings]").await,
            0,
            "no Settings tab repeats the visitor's fields"
        );
        c.ui.shot("visitor-access-view").await;
        edit_mode(c).await;
        c.ui.fill("#f-visitors-laptop-max-connections", "abc").await;
        wait_checked(c).await;
        c.ui.wait_for("[data-testid=form-problems] [data-problem]", SECS)
            .await;
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(
            c.ui.count("[data-testid=form-problems] [data-problem]")
                .await,
            1,
            "one problem in one field is listed once"
        );
        let badge =
            c.ui.text("[role=tablist] [data-tab=access] [data-tab-errors]")
                .await;
        assert_eq!(
            badge.split_whitespace().collect::<Vec<_>>(),
            ["1", "problem"],
            "the tab counts it once"
        );
        c.ui.click("[data-testid=discard-edit]").await;
        c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
        edit_mode(c).await;
        assert_eq!(
            c.ui.count("[data-testid=form-problems]").await,
            0,
            "a discarded edit leaves no problems behind"
        );
        let reach = "[data-schema='visitors/*/reach']";
        c.ui.wait_for(
            &format!("{reach} [data-key=home] input[value=only]:checked"),
            SECS,
        )
        .await;
        c.ui.wait_for(&format!("{reach} [data-chip='127.0.0.1/32']"), SECS)
            .await;
        click_js(c, &format!("{reach} [data-key=home] input[value=any]")).await;
        c.ui.wait_gone(&format!("{reach} [data-chip]"), SECS).await;
        wait_checked(c).await;
        c.ui.shot("reach-any-target").await;
        click_js(c, &format!("{reach} [data-key=home] input[value=only]")).await;
        c.ui.wait_for(&format!("{reach} [data-chip='127.0.0.1/32']"), SECS)
            .await;
        assert_eq!(
            c.ui.count("#f-visitors-laptop-reach-add option[value=self]")
                .await,
            0,
            "this server is not offered while it has no allowed targets"
        );
        c.ui.fill(
            &format!("{reach} [data-key=home] input[type=text]"),
            "127.0.0.1/32:22",
        )
        .await;
        c.ui.press(
            &format!("{reach} [data-key=home] input[type=text]"),
            "Enter",
        )
        .await;
        c.ui.wait_for(&format!("{reach} [data-chip='127.0.0.1/32:22']"), SECS)
            .await;
        let geo: Vec<f64> = c
            .ui
            .eval(&format!("(() => {{ const r = document.querySelector(\"{reach} [data-key=home]\").getBoundingClientRect(); const b = document.querySelector(\"{reach} [data-key=home] button[aria-label='Remove home']\").getBoundingClientRect(); return [r.right, b.right]; }})()"))
            .await;
        assert!(
            geo[1] <= geo[0] + 0.5,
            "Remove stays inside its row: {geo:?}"
        );
        c.ui.shot("reach-only-these").await;
        c.ui.click("[data-testid=discard-edit]").await;
        c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
        edit_mode(c).await;
        c.ui.fill("#f-visitors-laptop-max-connections", "50").await;
        wait_checked(c).await;
        assert_eq!(
            c.ui.count("[data-testid=review-open]").await,
            0,
            "an edit not yet added to the changes is not counted"
        );
        c.ui.click("[data-testid=save]").await;
        c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
        c.ui.wait_text("[data-testid=review-count]", "1", SECS)
            .await;
        edit_mode(c).await;
        c.ui.fill("#f-visitors-laptop-max-connections", "60").await;
        wait_checked(c).await;
        c.ui.click("[data-testid=review-open]").await;
        c.ui.wait_for("[data-testid=review-diff]", SECS).await;
        let diff = c.ui.text("[data-testid=review-diff]").await;
        assert!(
            diff.contains("max_connections: 50") && !diff.contains("max_connections: 60"),
            "the review shows only what was added: {diff}"
        );
        let _: bool =
            c.ui.eval("(() => { history.back(); return true; })()")
                .await;
        c.ui.wait_for("[data-testid=save-bar]", SECS).await;
        c.ui.wait_for("[role=tablist] [data-tab=access][aria-selected=true]", SECS)
            .await;
        let kept: String =
            c.ui.eval("document.getElementById('f-visitors-laptop-max-connections').value")
                .await;
        assert_eq!(kept, "60", "Back returns to the edit with its value");
        c.ui.shot("edit-kept-after-back").await;
        c.ui.nav("/services/web").await;
        c.ui.wait_text("[data-testid=entity-list]", "web", SECS)
            .await;
        c.ui.nav("/visitors").await;
        c.ui.wait_text(
            "[data-testid=entity-list] [data-row=laptop]",
            "unsaved",
            SECS,
        )
        .await;
        c.ui.nav("/visitors/laptop").await;
        c.ui.wait_for("[data-testid=save-bar]", SECS).await;
        c.ui.click("[data-testid=discard-edit]").await;
        c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
        c.ui.wait_for(
            "[data-testid=entity-list] [data-row=ops][data-group='In use now']",
            SECS,
        )
        .await;
        c.ui.nav("/visitors/ops").await;
        c.ui.wait_for("main h1 [data-testid=you-tag]", SECS).await;
        edit_mode(c).await;
        let admin = c.ui.text("[data-schema='visitors/*/admin']").await;
        assert!(
            admin.matches("burrow admin").count() == 1 && !admin.contains("Everything read gives"),
            "the admin field explains itself once: {admin}"
        );
        c.ui.click("[data-testid=discard-edit]").await;
        c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
        assert!(
            c.ui.text("main h1").await.contains("admin · write"),
            "the visitor title says its admin level and that it is you"
        );
        c.ui.nav("/clients/home").await;
        let row =
            c.ui.wait_text("[data-testid=entity-list] [data-row=home]", " · for ", SECS)
                .await;
        assert!(
            row.contains("127.0.0.1"),
            "an online client row says where from and for how long: {row}"
        );
        edit_mode(c).await;
        assert!(
            !c.ui.enabled("[data-testid=save]").await,
            "nothing to add before a change"
        );
        tokio::time::sleep(Duration::from_secs(6)).await;
        assert_eq!(
            c.ui.count("[data-testid=save-bar]").await,
            1,
            "the periodic reload does not end the edit"
        );
        c.ui.click("[data-testid=discard-edit]").await;
        c.ui.wait_gone("[data-testid=save-bar]", SECS).await;

        c.ui.nav("/services/web").await;
        c.ui.wait_for(
            "[data-testid=entity-list] [data-row=web][data-selected]",
            SECS,
        )
        .await;
        assert!(
            c.ui.text("main h1").await.contains("TCP"),
            "the page title carries the protocol tag"
        );
        let meta: String = c
            .ui
            .eval("document.querySelector('[data-testid=entity-list] [data-row=web]').lastElementChild.textContent")
            .await;
        assert_eq!(
            meta, "0 open",
            "every row shows its open count, the active one too"
        );
        let tags = c.ui.text("[data-testid=entity-list] [data-row=web]").await;
        assert!(
            tags.contains("TCP"),
            "TCP rows are tagged like UDP ones: {tags}"
        );
        c.ui.click("[role=tablist] [data-tab=settings]").await;
        let protocol = c.ui.text("[data-schema='services/*/protocol']").await;
        assert!(
            protocol.contains("TCP")
                && protocol.contains("(default)")
                && !protocol.contains("Default:"),
            "a default reads as a plain value: {protocol}"
        );
        edit_mode(c).await;
        c.ui.choose("f-services-web-bind-visibility", "private")
            .await;
        c.ui.wait_for("[data-testid=pick-visitors]", SECS).await;
        wait_checked(c).await;
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(
            c.ui.count("[data-testid=form-problems]").await,
            0,
            "the hint asks for visitors; no red error while they are being picked"
        );
        assert!(
            !c.ui.enabled("[data-testid=save]").await,
            "a private service without visitors cannot be added"
        );
        assert_eq!(
            c.ui.text("[data-testid=fields-changed]").await,
            "Pick at least one visitor or switch back to Public port"
        );
        c.ui.shot("private-needs-visitors").await;
        c.ui.click("[data-testid=pick-visitors] button").await;
        c.ui.wait_for(
            "#f-services-web-visitors:focus, #f-services-web-visitors:focus-within",
            SECS,
        )
        .await;
        clear_of_bar(c, "services/*/visitors").await;
        c.ui.fill("#f-services-web-max-connections", "abc").await;
        c.ui.wait_for("[data-testid=form-problems] [data-problem]", SECS)
            .await;
        c.ui.wait_for("[role=tablist] [data-tab=settings] [data-tab-errors]", SECS)
            .await;
        c.ui.click("[data-testid=form-problems] button").await;
        c.ui.wait_for(
            "#f-services-web-max-connections:focus, #f-services-web-max-connections:focus-within",
            SECS,
        )
        .await;
        c.ui.click("[data-testid=discard-edit]").await;
        c.ui.wait_gone("[data-testid=save-bar]", SECS).await;

        c.ui.nav("/clients/home?tab=settings").await;
        edit_mode(c).await;
        c.ui.fill("#f-clients-home-max-connections", "7").await;
        wait_checked(c).await;
        c.ui.click("[data-testid=save]").await;
        c.ui.wait_for("[data-testid=toast]", SECS).await;
        c.ui.click("[data-testid=review-open]").await;
        c.ui.wait_for("[data-testid=apply]", SECS).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let gap: f64 = c
            .ui
            .eval("(() => { const t = document.querySelector('[data-testid=toasts]').getBoundingClientRect(); const b = document.querySelector('[data-sticky-bar]').getBoundingClientRect(); return b.top - t.bottom; })()")
            .await;
        assert!(
            gap >= 0.0,
            "a toast sits above the review's action bar: {gap}"
        );
        c.ui.shot("toast-above-the-action-bar").await;

        c.ui.nav("/services/web?tab=settings").await;
        edit_mode(c).await;
        c.ui.fill("#f-services-web-max-connections", "9").await;
        wait_checked(c).await;
        c.ui.choose("f-services-web-protocol", "udp").await;
        let clear = "[data-schema='services/*/max_connections'] [data-testid=clear-unused]";
        c.ui.wait_for(clear, SECS).await;
        assert!(
            c.ui.text(clear)
                .await
                .contains("UDP services do not use this"),
            "a value the protocol cannot use can be cleared in place"
        );
        c.ui.click(clear).await;
        c.ui.wait_gone(clear, SECS).await;
        wait_checked(c).await;
        assert_eq!(c.ui.count("[data-testid=form-problems]").await, 0);
        c.ui.choose("f-services-web-bind-visibility", "private")
            .await;
        c.ui.wait_for("[data-testid=pick-visitors]", SECS).await;
        wait_checked(c).await;
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(
            c.ui.count("[data-testid=form-problems]").await,
            0,
            "a private UDP service asks for visitors like a TCP one, with no red error"
        );
        assert_eq!(
            c.ui.text("[data-testid=fields-changed]").await,
            "Pick at least one visitor or switch back to Public port"
        );
        c.ui.shot("private-udp-needs-visitors").await;
        c.ui.click("[data-testid=pick-visitors] button").await;
        c.ui.wait_for(
            "#f-services-web-visitors:focus, #f-services-web-visitors:focus-within",
            SECS,
        )
        .await;
        clear_of_bar(c, "services/*/visitors").await;
        let _: bool = c
            .ui
            .eval("(() => { const f = document.getElementById('f-services-web-visitors'); const i = f.matches('input') ? f : f.querySelector('input'); i.focus(); i.click(); return true; })()")
            .await;
        c.ui.wait_for("[role=listbox] [role=option]", SECS).await;
        c.ui.click("[role=listbox] [role=option]").await;
        c.ui.wait_for("[data-chip]", SECS).await;
        wait_checked(c).await;
        assert_eq!(c.ui.count("[data-testid=form-problems]").await, 0);
        assert!(
            c.ui.enabled("[data-testid=save]").await,
            "a private UDP service with a visitor can be added"
        );
        c.ui.shot("private-udp-with-a-visitor").await;
        c.ui.click("[data-testid=discard-edit]").await;
        c.ui.wait_gone("[data-testid=save-bar]", SECS).await;

        c.ui.viewport(420, 700).await;
        c.ui.nav("/visitors").await;
        c.ui.click("[data-testid=new]").await;
        c.ui.wait_for("#new-links", SECS).await;
        let placeholder: String =
            c.ui.eval("document.getElementById('new-links').placeholder")
                .await;
        assert!(!placeholder.is_empty(), "the picker says what it is for");
        let before: f64 = c
            .ui
            .eval("(() => { scrollTo(0, 0); const r = document.getElementById('new-links').getBoundingClientRect(); const b = document.querySelector('[data-sticky-bar]').getBoundingClientRect(); scrollBy(0, r.bottom - b.top + 8); return scrollY; })()")
            .await;
        let _: bool = c
            .ui
            .eval("(() => { const e = document.getElementById('new-links'); e.focus(); e.click(); return true; })()")
            .await;
        c.ui.wait_for("[role=listbox]", SECS).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        let seen: Vec<f64> = c
            .ui
            .eval("(() => { const l = document.querySelector('[role=listbox]').parentElement.getBoundingClientRect(); const b = document.querySelector('[data-sticky-bar]').getBoundingClientRect(); return [l.top, l.bottom, b.top, scrollY]; })()")
            .await;
        assert!(
            (seen[3] - before).abs() < 1.0,
            "opening the picker does not scroll the page: {before} then {seen:?}"
        );
        assert!(
            seen[0] >= 0.0 && seen[1] <= seen[2] + 0.5,
            "the open list fits between the top and the save bar: {seen:?}"
        );
        assert_eq!(
            c.ui.count("[role=listbox]").await,
            1,
            "the picker stays open after the click"
        );
        c.ui.click("[role=listbox] [role=option]").await;
        c.ui.wait_for("#new-links-chips [data-chip], [data-chip]", SECS)
            .await;
        c.ui.shot("new-visitor-picker-short-window").await;
        c.ui.press("#new-links", "Escape").await;
        c.ui.click("[data-testid=create-cancel]").await;
        c.ui.nav("/services/web").await;
        c.ui.wait_for("[data-testid=health] table", SECS).await;
        let wide: bool = c
            .ui
            .eval("(() => { const w = document.querySelector('[data-testid=health] table').parentElement; return w.scrollWidth > w.clientWidth + 1; })()")
            .await;
        assert!(
            !wide,
            "Health per client fits a narrow screen without scrolling sideways"
        );
        c.ui.viewport(1440, 900).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn edits_survive_navigation_and_problems_show_where_you_edit() {
    run("edit-state", Options::default(), false, edit_state);
}
