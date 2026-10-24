use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use super::world::{Options, free_range};
use super::{
    Ctx, SECS, applied, echo_server, edit_mode, open, open_review, round_trip, run, wait_checked,
};

pub(super) type Flow<'a> = Pin<Box<dyn Future<Output = ()> + 'a>>;

pub(super) const UNSET: &str = "\u{1}unset";

pub(super) async fn discard_from_review(c: &mut Ctx) {
    c.ui.click("[data-testid=discard]").await;
    c.ui.wait_gone("[data-testid=review-open]", SECS).await;
}

fn add_service(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        let echo = free_range(3);
        let public = free_range(3);
        let mut echoes = Vec::new();
        for i in 0..3 {
            echoes.push(echo_server(echo + i).await);
        }
        open(c, "ops").await;
        c.ui.nav("/services").await;
        c.ui.shot("services-list").await;
        c.ui.click("[data-testid=new]").await;
        c.ui.click("[data-testid=new-menu] [data-new=service]")
            .await;
        c.ui.wait_for("[data-testid=new-entity]", SECS).await;
        c.ui.shot("new-service-dialog").await;
        c.ui.fill("#new-name", "has space").await;
        c.ui.wait_text("#new-name-msg", "", SECS).await;
        assert!(
            !c.ui.enabled("[data-testid=create]").await,
            "an invalid name cannot be created"
        );
        c.ui.shot("invalid-name").await;
        c.ui.fill("#new-name", "game").await;
        c.ui.wait_for("[data-testid=name-available]", SECS).await;
        c.ui.click("[data-testid=create]").await;
        c.ui.wait_for("[data-testid=detail][data-name=game]", SECS)
            .await;
        c.ui.wait_for("[data-testid=save-bar]", SECS).await;
        c.ui.shot("new-service-form").await;

        assert_eq!(
            c.ui.count("[data-schema='services/*/client'][data-path]")
                .await,
            0,
            "client and clients are one picker"
        );
        c.ui.wait_for("[data-schema='services/*/clients'] [data-chip=home]", SECS)
            .await;
        c.ui.click("#f-services-game-clients").await;
        assert_eq!(
            c.ui.count("[data-schema='services/*/mode']").await,
            0,
            "mode needs two clients"
        );
        c.ui.click("[data-schema='services/*/clients'] [data-option=office] input")
            .await;
        c.ui.wait_for("[data-schema='services/*/mode']", SECS).await;
        c.ui.shot("two-clients-show-mode").await;
        assert_eq!(
            c.ui.count("#f-services-game-bind").await,
            0,
            "a new service starts private"
        );
        click_js(
            c,
            "[data-schema='services/*/visibility'] input[value=public]",
        )
        .await;

        c.ui.fill(
            "#f-services-game-bind",
            &format!("127.0.0.1:{}-{}", public, public + 2),
        )
        .await;
        c.ui.fill(
            "#f-services-game-target",
            &format!("127.0.0.1:{}-{}", echo, echo + 1),
        )
        .await;
        c.ui.wait_text(
            "[data-schema='services/*/bind'] [data-testid=port-count]",
            "3 ports",
            SECS,
        )
        .await;
        let host: String =
            c.ui.eval("document.getElementById('f-services-game-bind-host').value")
                .await;
        assert_eq!(host, "127.0.0.1", "a pasted address fills the address part");
        stored_as(
            c,
            "services/*/bind",
            &format!("127.0.0.1:{}-{}", public, public + 2),
        )
        .await;
        let row = c.ui.text("[data-schema='services/*/bind']").await;
        assert!(
            row.contains("Saved as")
                && row.contains("up to 1000 ports")
                && !row.contains("for example"),
            "one line says how it is saved instead of a second help line: {row}"
        );
        c.ui.wait_text(
            "[data-schema='services/*/target']",
            "2 ports here, but bind has 3",
            SECS,
        )
        .await;
        c.ui.wait_for("[data-testid=form-problems]", SECS).await;
        assert!(
            !c.ui.enabled("[data-testid=save]").await,
            "a local error blocks saving the edit"
        );
        c.ui.shot("range-size-mismatch").await;
        c.ui.fill(
            "#f-services-game-target",
            &format!("127.0.0.1:{}-{}", echo, echo + 2),
        )
        .await;
        wait_checked(c).await;
        let target = c.ui.text("[data-schema='services/*/target']").await;
        assert!(!target.contains("ports here"), "{target}");
        c.ui.shot("ranges-match").await;
        c.ui.click("[data-testid=save]").await;
        c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
        c.ui.wait_text("[data-testid=toasts]", "Added to changes", SECS)
            .await;

        open_review(c, "review-new-service").await;
        let changes = c.ui.text("[data-testid=review-list]").await;
        assert!(changes.contains("game"), "{changes}");
        assert_eq!(c.ui.count("[data-testid=review-diff]").await, 1);
        c.ui.wait_for("[data-review-path=game]", SECS).await;
        applied(c, "applied").await;
        for i in 0..3 {
            round_trip(public + i, 4096)
                .await
                .expect("a round trip through each public port");
        }
        let text = c.world.config();
        assert!(text.contains("# the home box"), "comments survive:\n{text}");
        assert!(
            text.contains("# keep this comment"),
            "comments survive:\n{text}"
        );
        assert!(text.contains("game:"), "{text}");
        assert!(
            text.contains(&format!("bind: 127.0.0.1:{}-{}", public, public + 2)),
            "{text}"
        );
        assert!(
            text.contains(&format!("target: 127.0.0.1:{}-{}", echo, echo + 2)),
            "{text}"
        );
        c.ui.shot("service-live").await;

        c.ui.nav("/services/web").await;
        c.ui.nav("/services/game").await;
        edit_mode(c).await;
        c.ui.fill(
            "#f-services-game-bind",
            &format!("127.0.0.1:{}", c.world.ports["web"]),
        )
        .await;
        wait_checked(c).await;
        c.ui.wait_text(
            "[data-schema='services/*/bind'] [aria-live]",
            "already",
            SECS,
        )
        .await;
        c.ui.wait_for("[data-testid=check-state][data-state=errors]", SECS)
            .await;
        assert!(
            !c.ui.enabled("[data-testid=save]").await,
            "a server error blocks saving"
        );
        c.ui.shot("server-says-duplicate-bind").await;
        c.ui.click("[data-schema='services/*/bind'] [data-testid=reset-field]")
            .await;
        c.ui.wait_gone("[data-testid=check-state][data-state=errors]", SECS)
            .await;
        c.ui.shot("undo-clears-the-error").await;
        for e in echoes {
            e.abort();
        }
        c.ui.assert_no_errors();
    })
}

#[test]
fn e3_e4_add_a_service_with_ranges_and_see_server_errors_inline() {
    run(
        "e3-e4-add-service",
        Options {
            clients: Some(vec!["home", "office"]),
            ..Options::default()
        },
        false,
        add_service,
    );
}

fn service_on_this_server(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        let echo = free_range(1);
        let public = free_range(1);
        let target = echo_server(echo).await;
        open(c, "ops").await;
        c.ui.nav("/services").await;
        c.ui.click("[data-testid=new]").await;
        c.ui.click("[data-testid=new-menu] [data-new=service]")
            .await;
        c.ui.wait_for("[data-testid=new-entity]", SECS).await;
        c.ui.fill("#new-name", "local").await;
        c.ui.wait_for("[data-testid=name-available]", SECS).await;
        c.ui.click("[data-testid=create]").await;
        c.ui.wait_for("[data-testid=detail][data-name=local]", SECS)
            .await;
        let clients = "[data-schema='services/*/clients']";
        c.ui.click(&format!("{clients} [data-remove]")).await;
        c.ui.click("#f-services-local-clients").await;
        c.ui.wait_for(&format!("{clients} [data-option=self]"), SECS)
            .await;
        let offered = c.ui.text(&format!("{clients} [data-option=self]")).await;
        assert!(offered.contains("This server"), "{offered}");
        c.ui.shot("this-server-in-the-picker").await;
        c.ui.click(&format!("{clients} [data-option=self] input"))
            .await;
        c.ui.wait_for(&format!("{clients} [data-chip=self]"), SECS)
            .await;
        click_js(
            c,
            "[data-schema='services/*/visibility'] input[value=public]",
        )
        .await;
        c.ui.fill("#f-services-local-bind", &format!("127.0.0.1:{public}"))
            .await;
        c.ui.fill("#f-services-local-target", &format!("127.0.0.1:{echo}"))
            .await;
        wait_checked(c).await;
        c.ui.click("[data-testid=save]").await;
        c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
        open_review(c, "review-service-on-this-server").await;
        applied(c, "service-on-this-server-applied").await;
        let text = c.world.config();
        assert!(text.contains("  local:\n    client: self\n"), "{text}");
        round_trip(public, 4096)
            .await
            .expect("a round trip through the server itself");
        c.ui.nav("/services/local").await;
        c.ui.wait_for("[data-hop=target]", SECS).await;
        let hops: Vec<String> = c
            .ui
            .eval("[...document.querySelectorAll('[data-testid=detail] [data-hop]')].map(e => e.getAttribute('data-hop'))")
            .await;
        assert_eq!(hops, ["internet", "server", "target"], "no client hop");
        c.ui.wait_text("[data-hop=target]", "on this server", SECS)
            .await;
        c.ui.shot("service-on-this-server-path").await;
        target.abort();
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_service_on_this_server_is_created_in_the_ui_and_carries_traffic() {
    run(
        "service-on-this-server",
        Options::default(),
        false,
        service_on_this_server,
    );
}

pub(super) async fn row_error(c: &Ctx, schema: &str) -> String {
    let css = format!("[data-schema='{schema}'] [aria-live] p");
    c.ui.wait_for(&css, SECS).await;
    c.ui.text(&css).await
}

pub(super) async fn clear_of_bar(c: &Ctx, schema: &str) {
    tokio::time::sleep(Duration::from_millis(400)).await;
    let gap: f64 = c
        .ui
        .eval(&format!(
            "(() => {{ const b = document.querySelector('[data-sticky-bar]'); const r = document.querySelector(\"[data-schema='{schema}']\"); return b.getBoundingClientRect().top - r.getBoundingClientRect().bottom; }})()"
        ))
        .await;
    assert!(gap >= 0.0, "the save bar covers {schema} by {gap}px");
}

pub(super) async fn stored_as(c: &Ctx, schema: &str, wanted: &str) {
    let css = format!("[data-schema='{schema}'] [data-testid=stored-as]");
    for _ in 0..50 {
        if c.ui.attr_of(&css, "data-stored").await == wanted {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!(
        "{schema} is not stored as {wanted:?}: {:?}",
        c.ui.attr_of(&css, "data-stored").await
    );
}

pub(super) async fn row_clean(c: &Ctx, schema: &str) {
    c.ui.wait_gone(&format!("[data-schema='{schema}'] [aria-live] p"), SECS)
        .await;
}

pub(super) async fn focus(c: &Ctx, id: &str) {
    let ok: bool = c
        .ui
        .eval(&format!(
            "(() => {{ const e = document.getElementById({id:?}); if (!e) return false; e.focus(); return true; }})()"
        ))
        .await;
    assert!(ok, "no #{id}");
}

pub(super) async fn click_js(c: &Ctx, css: &str) {
    let ok: bool = c
        .ui
        .eval(&format!(
            "(() => {{ const e = document.querySelector({css:?}); if (!e) return false; e.click(); return true; }})()"
        ))
        .await;
    assert!(ok, "nothing matches {css}");
}

pub(super) async fn open_more(c: &Ctx, schema: &str) {
    c.ui.wait_for(&format!("[data-schema='{schema}']"), SECS)
        .await;
    let _: bool = c
        .ui
        .eval(&format!(
            "(() => {{ const r = document.querySelector(\"[data-schema='{schema}']\").closest('[data-testid=more-region][hidden]'); if (r) document.querySelector('[aria-controls=\"' + r.id + '\"]').click(); return true; }})()"
        ))
        .await;
    tokio::time::sleep(Duration::from_millis(150)).await;
}
