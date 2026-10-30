use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use super::world::Options;
use super::{Ctx, SECS, ax_clean, edit_mode, open, run, wait_checked};

fn no_token(c: &mut Ctx) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    Box::pin(async move {
        let url = c.world.open_admin("ops");
        let bare = url.split('#').next().unwrap_or_default().to_string();
        c.ui.goto(&bare).await;
        c.ui.wait_for("[data-testid=no-token]", SECS).await;
        c.ui.shot("opened-without-the-link").await;
        ax_clean(c, "no-token").await;
        c.ui.goto(&url).await;
        c.ui.wait_gone("[data-testid=loading]", Duration::from_secs(20))
            .await;
        c.ui.wait_text("[data-testid=conn-pill]", "Connected", SECS)
            .await;
        c.ui.shot("then-with-the-link").await;
        c.ui.goto(&bare).await;
        c.ui.wait_text("[data-testid=conn-pill]", "Connected", SECS)
            .await;
        c.ui.shot("reload-keeps-the-session").await;
        c.ui.take_errors();
    })
}

#[test]
fn the_page_needs_the_link_from_the_terminal() {
    run("no-token", Options::default(), false, no_token);
}

async fn header_items(c: &Ctx) -> Vec<String> {
    c.ui.eval(
        "Array.from(document.querySelector('[data-testid=header-right]').children).filter(e => { const r = e.getBoundingClientRect(); return r.width > 2 && r.height > 2; }).map(e => e.getAttribute('data-testid') || e.querySelector('[data-testid]')?.getAttribute('data-testid') || e.tagName)",
    )
    .await
}

pub async fn popover_behaves(c: &Ctx, trigger: &str, panel: &str) {
    let t = format!("#{trigger}");
    let p = format!("#{panel}");
    c.ui.click(&t).await;
    c.ui.wait_for(&p, SECS).await;
    assert_eq!(c.ui.attr_of(&t, "aria-expanded").await, "true", "{trigger}");
    assert_eq!(c.ui.attr_of(&t, "aria-controls").await, panel, "{trigger}");
    let started = std::time::Instant::now();
    loop {
        let inside: bool = c
            .ui
            .eval(&format!(
                "(() => {{ const p = document.getElementById('{panel}'); return !!p && p.contains(document.activeElement); }})()"
            ))
            .await;
        if inside {
            break;
        }
        assert!(started.elapsed() < SECS, "focus moves into {panel}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let _: bool = c
        .ui
        .eval("(() => { document.activeElement.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); return true; })()")
        .await;
    c.ui.wait_gone(&p, SECS).await;
    let back: String =
        c.ui.eval("document.activeElement ? document.activeElement.id : ''")
            .await;
    assert_eq!(back, trigger, "Escape returns focus to the trigger");
    assert_eq!(
        c.ui.attr_of(&t, "aria-expanded").await,
        "false",
        "{trigger}"
    );
    c.ui.click(&t).await;
    c.ui.wait_for(&p, SECS).await;
    let _: bool = c
        .ui
        .eval("(() => { document.querySelector('main, #detail, body').dispatchEvent(new PointerEvent('pointerdown', { bubbles: true })); return true; })()")
        .await;
    c.ui.wait_gone(&p, SECS).await;
}

async fn theme_now(c: &Ctx) -> (String, String) {
    let t =
        c.ui.attr_of("[data-testid=theme-toggle]", "data-theme")
            .await;
    let class: String = c.ui.eval("document.documentElement.className").await;
    (t, class)
}

fn calm_header(c: &mut Ctx) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    Box::pin(async move {
        let url = c.world.open_admin("ops");
        c.ui.goto(&url).await;
        c.ui.wait_text("[data-testid=conn-pill]", "Connected to", SECS)
            .await;
        let items = header_items(c).await;
        assert_eq!(
            items,
            ["search-open", "theme-toggle", "avatar"],
            "the normal header has search, theme and the avatar only"
        );
        c.ui.shot("normal").await;

        assert_eq!(theme_now(c).await.0, "system");
        c.ui.click("[data-testid=theme-toggle]").await;
        let (t, class) = theme_now(c).await;
        assert!(t == "light" && class.contains("light"), "{t} {class}");
        c.ui.click("[data-testid=theme-toggle]").await;
        let (t, class) = theme_now(c).await;
        assert!(t == "dark" && class.contains("dark"), "{t} {class}");
        let bare = url.split('#').next().unwrap_or_default().to_string();
        c.ui.goto(&bare).await;
        c.ui.wait_text("[data-testid=conn-pill]", "Connected to", SECS)
            .await;
        let (t, class) = theme_now(c).await;
        assert!(
            t == "dark" && class.contains("dark"),
            "the theme survives a reload: {t} {class}"
        );
        c.ui.shot("dark-after-reload").await;
        c.ui.click("[data-testid=theme-toggle]").await;
        let (t, class) = theme_now(c).await;
        assert!(
            t == "system" && !class.contains("dark") && !class.contains("light"),
            "{t} {class}"
        );

        popover_behaves(c, "avatar", "avatar-menu").await;
        popover_behaves(c, "server-switcher", "server-menu").await;
        c.ui.click("[data-testid=avatar]").await;
        c.ui.wait_text("[data-testid=avatar-who]", "ops · admin write", SECS)
            .await;
        c.ui.wait_text("[data-testid=avatar-conn]", "Connected to", SECS)
            .await;
        c.ui.wait_for("[data-testid=avatar-about]", SECS).await;
        c.ui.shot("avatar-menu").await;
        c.ui.click("[data-testid=avatar]").await;

        c.ui.nav("/services/web").await;
        c.ui.wait_for("#entity-more", SECS).await;
        popover_behaves(c, "entity-more", "entity-more-menu").await;
        edit_mode(c).await;
        c.ui.fill("#f-services-web-target", "4321").await;
        wait_checked(c).await;
        c.ui.click("[data-testid=save]").await;
        c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
        c.ui.wait_for("[data-testid=review-open]", SECS).await;
        assert_eq!(
            header_items(c).await,
            ["search-open", "theme-toggle", "review-open", "avatar"],
            "Review joins the header only with pending changes"
        );

        c.world.stop("server");
        c.ui.wait_text(
            "[data-testid=conn-pill]",
            "Reconnecting in",
            Duration::from_secs(15),
        )
        .await;
        assert_eq!(
            header_items(c).await,
            [
                "search-open",
                "theme-toggle",
                "conn-pill",
                "review-open",
                "avatar"
            ],
            "the reconnecting text shows only while reconnecting"
        );
        popover_behaves(c, "conn-pill", "retry-popover").await;
        c.ui.click("[data-testid=conn-pill]").await;
        c.ui.wait_for("[data-testid=retry-now]", SECS).await;
        c.ui.shot("reconnecting").await;
        c.world
            .start("server", &["server", "-c", "server.yaml"], &[]);
        c.ui.wait_text(
            "[data-testid=conn-pill]",
            "Connected to",
            Duration::from_secs(30),
        )
        .await;
        assert_eq!(
            header_items(c).await,
            ["search-open", "theme-toggle", "review-open", "avatar"],
            "the reconnecting text goes away once connected"
        );
        c.ui.take_errors();
    })
}

fn phone_header(c: &mut Ctx) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.viewport(390, 900).await;
        c.ui.nav("/services/web").await;
        c.ui.wait_for("[data-testid=mobile-back]", SECS).await;
        let line = c.ui.text("[data-testid=server-line]").await;
        assert!(
            line.contains("127.0.0.1"),
            "the phone header names the server: {line}"
        );
        let back = c.ui.text("[data-testid=mobile-back]").await;
        assert!(
            !back.contains("web"),
            "the header names the server, not the page: {back}"
        );
        let crumb_hidden: bool = c
            .ui
            .eval("[...document.querySelectorAll('nav[aria-label=Breadcrumb]')].every(n => n.offsetParent === null)")
            .await;
        assert!(
            crumb_hidden,
            "the name shows once, as the title, not again in a breadcrumb"
        );
        c.ui.shot("phone-detail-header").await;
        c.ui.viewport(1440, 900).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn the_phone_header_names_the_server_on_every_page() {
    run("phone-header", Options::default(), false, phone_header);
}

#[test]
fn the_header_is_calm_in_every_state() {
    run("header", Options::default(), false, calm_header);
}

fn read_only(c: &mut Ctx) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    Box::pin(async move {
        open(c, "viewer").await;
        c.ui.wait_text("[data-testid=conn-pill]", "Connected", SECS)
            .await;
        c.ui.wait_text("[data-testid=you-level]", "viewer (read)", SECS)
            .await;
        c.ui.wait_for("[data-testid=read-only-tag]", SECS).await;
        assert_eq!(
            c.ui.count("[data-testid=banners] [data-testid^=banner]")
                .await,
            0,
            "read-only shows no banner"
        );
        assert_eq!(
            header_items(c).await,
            ["search-open", "theme-toggle", "read-only-tag", "avatar"],
            "the read-only tag takes the Review button's place"
        );
        popover_behaves(c, "read-only-tag", "read-only-why").await;
        c.ui.click("[data-testid=read-only-tag]").await;
        c.ui.wait_text("[data-testid=read-only-why]", "has read admin", SECS)
            .await;
        c.ui.shot("read-only-overview").await;
        c.ui.click("[data-testid=read-only-tag]").await;
        c.ui.wait_gone("[data-testid=read-only-why]", SECS).await;
        c.ui.nav("/settings").await;
        c.ui.shot("read-only-settings").await;
        ax_clean(c, "read-only").await;

        #[cfg(unix)]
        {
            let file = c.world.file("server.yaml");
            let mut perms = std::fs::metadata(&file).expect("server.yaml").permissions();
            std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o444);
            std::fs::set_permissions(&file, perms.clone()).expect("make server.yaml read-only");
            let url = c.world.open_admin("ops");
            c.ui.goto("about:blank").await;
            c.ui.goto(&url).await;
            c.ui.wait_for("[data-testid=read-only-tag]", SECS).await;
            c.ui.wait_text("[data-testid=conn-pill]", "Connected", SECS)
                .await;
            assert_eq!(
                c.ui.count("[data-testid=banners] [data-testid^=banner]")
                    .await,
                0,
                "a config the server cannot write shows no banner"
            );
            c.ui.click("[data-testid=read-only-tag]").await;
            c.ui.wait_text("[data-testid=read-only-why]", "cannot write", SECS)
                .await;
            c.ui.wait_text("[data-testid=read-only-why]", "write access to", SECS)
                .await;
            c.ui.shot("not-writable-reason").await;
            std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o600);
            std::fs::set_permissions(&file, perms).expect("make server.yaml writable again");
        }
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_read_only_admin_sees_everything_and_is_told_why_it_cannot_change_it() {
    run("read-only", Options::default(), false, read_only);
}

const HEADER_MEASURED: &str = "Math.abs(parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--hdr-h')) - document.querySelector('[data-testid=header]').getBoundingClientRect().height) < 0.5";

fn header_height(c: &mut Ctx) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/services/web").await;
        for width in [1100, 390, 1440] {
            c.ui.viewport(width, 900).await;
            c.ui.wait_true(
                HEADER_MEASURED,
                "--hdr-h follows the header's height on a normal load",
                SECS,
            )
            .await;
        }
        c.ui.viewport(1100, 900).await;
        c.ui.wait_true(HEADER_MEASURED, "--hdr-h at 1100 px", SECS)
            .await;
        c.ui.wait_true(
            "Math.abs(parseFloat(getComputedStyle(document.querySelector('[data-testid=list]').closest('aside')).top) - document.querySelector('[data-testid=header]').getBoundingClientRect().height) < 0.5",
            "the sticky list's offset is the header's real height between lg and xl",
            SECS,
        )
        .await;
        c.ui.wait_true(
            "document.documentElement.scrollHeight <= innerHeight",
            "a short page fits under the real header, so the list never slides beneath it",
            SECS,
        )
        .await;
        c.ui.click("[role=tablist] [data-tab=settings]").await;
        c.ui.wait_for("[data-schema='services/*/target']", SECS)
            .await;
        let tall: bool =
            c.ui.eval("document.documentElement.scrollHeight > innerHeight + 300")
                .await;
        assert!(
            tall,
            "the settings page is taller than the window, so the page scrolls"
        );
        let _: bool = c.ui.eval("(window.scrollTo(0, 600), true)").await;
        c.ui.wait_true(
            "document.querySelector('[data-testid=list]').closest('aside').getBoundingClientRect().top >= document.querySelector('[data-testid=header]').getBoundingClientRect().bottom - 1",
            "the sticky list sits below the header between lg and xl",
            SECS,
        )
        .await;
        c.ui.viewport(1440, 900).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn the_header_height_is_measured_from_the_start_at_every_width() {
    run("header-height", Options::default(), false, header_height);
}

fn search_modal(c: &mut Ctx) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    Box::pin(async move {
        open(c, "ops").await;
        let _: bool = c
            .ui
            .eval("(document.querySelector('[data-testid=search-open]').focus(), document.querySelector('[data-testid=search-open]').click(), true)")
            .await;
        c.ui.wait_for("[data-testid=search] input", SECS).await;
        c.ui.wait_true(
            "document.activeElement && document.activeElement.closest('[data-testid=search]') !== null",
            "the search opens with focus inside it",
            SECS,
        )
        .await;
        for _ in 0..30 {
            c.ui.press("[data-testid=search]", "Tab").await;
        }
        let inside: bool = c
            .ui
            .eval("document.activeElement && document.activeElement.closest('[data-testid=search]') !== null")
            .await;
        assert!(inside, "Tab stays inside the search");
        c.ui.press("[data-testid=search]", "Escape").await;
        c.ui.wait_gone("[data-testid=search]", SECS).await;
        c.ui.wait_true(
            "document.activeElement && document.activeElement.dataset.testid === 'search-open'",
            "closing the search gives focus back to what opened it",
            SECS,
        )
        .await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn the_search_keeps_focus_inside_and_gives_it_back() {
    run("search-modal", Options::default(), false, search_modal);
}
