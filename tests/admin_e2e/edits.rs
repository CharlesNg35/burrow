use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use super::world::{Options, free_range};
use super::{
    Ctx, SECS, applied, echo_server, edit_mode, open, open_review, round_trip, run, wait_checked,
};

type Flow<'a> = Pin<Box<dyn Future<Output = ()> + 'a>>;

const UNSET: &str = "\u{1}unset";

async fn discard_from_review(c: &mut Ctx) {
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

async fn row_error(c: &Ctx, schema: &str) -> String {
    let css = format!("[data-schema='{schema}'] [aria-live] p");
    c.ui.wait_for(&css, SECS).await;
    c.ui.text(&css).await
}

async fn clear_of_bar(c: &Ctx, schema: &str) {
    tokio::time::sleep(Duration::from_millis(400)).await;
    let gap: f64 = c
        .ui
        .eval(&format!(
            "(() => {{ const b = document.querySelector('[data-sticky-bar]'); const r = document.querySelector(\"[data-schema='{schema}']\"); return b.getBoundingClientRect().top - r.getBoundingClientRect().bottom; }})()"
        ))
        .await;
    assert!(gap >= 0.0, "the save bar covers {schema} by {gap}px");
}

async fn stored_as(c: &Ctx, schema: &str, wanted: &str) {
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

async fn row_clean(c: &Ctx, schema: &str) {
    c.ui.wait_gone(&format!("[data-schema='{schema}'] [aria-live] p"), SECS)
        .await;
}

async fn focus(c: &Ctx, id: &str) {
    let ok: bool = c
        .ui
        .eval(&format!(
            "(() => {{ const e = document.getElementById({id:?}); if (!e) return false; e.focus(); return true; }})()"
        ))
        .await;
    assert!(ok, "no #{id}");
}

async fn click_js(c: &Ctx, css: &str) {
    let ok: bool = c
        .ui
        .eval(&format!(
            "(() => {{ const e = document.querySelector({css:?}); if (!e) return false; e.click(); return true; }})()"
        ))
        .await;
    assert!(ok, "nothing matches {css}");
}

fn editors(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        let public = free_range(2);
        open(c, "ops").await;
        c.ui.nav("/services/web").await;
        edit_mode(c).await;

        let bind = "services/*/bind";
        c.ui.fill("#f-services-web-bind", "70000").await;
        let err = row_error(c, bind).await;
        assert!(!err.is_empty(), "port 70000 is refused inline");
        c.ui.shot("bind-port-out-of-range").await;
        c.ui.fill("#f-services-web-bind", &format!("127.0.0.1:{public}"))
            .await;
        row_clean(c, bind).await;
        c.ui.wait_text(
            "[data-schema='services/*/bind'] [data-testid=port-count]",
            "1 port",
            SECS,
        )
        .await;
        c.ui.select("#f-services-web-bind-host", "\u{1}specific")
            .await;
        c.ui.fill("#f-services-web-bind-host-ip", "999.1.1.1").await;
        let err = row_error(c, bind).await;
        assert!(!err.is_empty(), "a bad IP is refused inline");
        c.ui.shot("bind-bad-ip").await;
        c.ui.select("#f-services-web-bind-host", "127.0.0.1").await;
        row_clean(c, bind).await;

        let target = "services/*/target";
        click_js(c, "[data-schema='services/*/target'] input[value=unix]").await;
        c.ui.fill("#f-services-web-target", "/tmp/burrow-e2e.sock")
            .await;
        stored_as(c, "services/*/target", "unix:/tmp/burrow-e2e.sock").await;
        c.ui.shot("target-unix-socket").await;
        click_js(c, "[data-schema='services/*/target'] input[value=network]").await;
        c.ui.fill("#f-services-web-target-host", "127.0.0.1").await;
        c.ui.fill("#f-services-web-target", "abc").await;
        let err = row_error(c, target).await;
        assert!(!err.is_empty(), "a bad target port is refused inline");
        c.ui.fill("#f-services-web-target", &c.world.ports["echo"].to_string())
            .await;
        row_clean(c, target).await;

        click_js(
            c,
            "[data-schema='services/*/health_check'] input[value=custom]",
        )
        .await;
        c.ui.wait_for("[data-testid=custom-box]", SECS).await;
        c.ui.fill("#f-services-web-health-check-interval-secs", "0")
            .await;
        let err = row_error(c, "services/*/health_check/interval_secs").await;
        assert!(!err.is_empty(), "an interval of 0 is refused inline");
        c.ui.shot("health-check-custom-invalid").await;
        c.ui.fill("#f-services-web-health-check-interval-secs", "5")
            .await;
        row_clean(c, "services/*/health_check/interval_secs").await;
        c.ui.shot("health-check-custom").await;

        let client = "[data-schema='services/*/clients']";
        c.ui.click(&format!("{client} [data-remove]")).await;
        c.ui.wait_for("[data-testid=form-problems]", SECS).await;
        c.ui.shot("no-client-is-a-problem").await;
        c.ui.fill("#f-services-web-clients", "off").await;
        focus(c, "f-services-web-clients").await;
        assert_eq!(
            c.ui.count(&format!("{client} [data-option]")).await,
            1,
            "typing filters the clients"
        );
        c.ui.shot("picker-filtered").await;
        c.ui.fill("#f-services-web-clients", "ho").await;
        focus(c, "f-services-web-clients").await;
        c.ui.press("#f-services-web-clients", "Enter").await;
        c.ui.wait_for(&format!("{client} [data-chip=home]"), SECS)
            .await;
        wait_checked(c).await;
        c.ui.click("[data-testid=save]").await;
        open_review(c, "review-of-the-editors").await;
        applied(c, "editors-applied").await;
        let text = c.world.config();
        assert!(
            text.contains(&format!("bind: 127.0.0.1:{public}")),
            "{text}"
        );
        assert!(text.contains("interval_secs: 5"), "{text}");

        c.ui.nav("/settings").await;
        edit_mode(c).await;
        c.ui.wait_for("[data-schema='pool/min_idle'] [data-editor]", SECS)
            .await;
        let _: bool = c
            .ui
            .eval("(() => { const b = document.querySelector('[data-sticky-bar]').getBoundingClientRect(); const r = document.querySelector(\"[data-schema='pool/min_idle']\").getBoundingClientRect(); scrollBy(0, r.bottom - b.top - 10); document.getElementById('f-pool-min-idle').focus(); return true; })()")
            .await;
        clear_of_bar(c, "pool/min_idle").await;
        c.ui.fill("#f-pool-min-idle", "100").await;
        let err = row_error(c, "pool/min_idle").await;
        assert!(err.contains("max_idle"), "{err}");
        clear_of_bar(c, "pool/min_idle").await;
        assert!(
            !c.ui.enabled("[data-testid=save]").await,
            "a server error in a setting blocks Add to changes"
        );
        c.ui.click("[data-testid=discard-edit]").await;
        c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
        edit_mode(c).await;
        c.ui.wait_for("[data-server-address]", SECS).await;
        click_js(c, "[data-server-address] input[value=host]").await;
        c.ui.fill("#f-public-address", "bad host").await;
        let err = row_error(c, "public_address").await;
        assert!(!err.is_empty(), "a host with a space is refused inline");
        c.ui.shot("server-address-invalid").await;
        click_js(c, "[data-server-address] input[value=wss]").await;
        c.ui.fill("#f-public-address", "tunnel.example.net/burrow")
            .await;
        let err = row_error(c, "public_address").await;
        assert!(
            err.contains("no websocket section"),
            "a wss:// address is refused inline on a server without WebSocket, as validate refuses it: {err}"
        );
        c.ui.shot("server-address-wss").await;
        click_js(c, "[data-server-address] input[value=host]").await;
        c.ui.fill("#f-public-address", "tunnel.example.net").await;
        row_clean(c, "public_address").await;
        open_review(c, "review-of-the-server-address").await;
        applied(c, "server-address-applied").await;
        let text = c.world.config();
        assert!(
            text.contains("public_address: tunnel.example.net"),
            "{text}"
        );
        c.ui.assert_no_errors();
    })
}

#[test]
fn the_address_health_check_and_picker_editors_check_input_and_write_yaml() {
    run("editors", Options::default(), false, editors);
}

fn conflict(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/services/web").await;
        edit_mode(c).await;
        c.ui.fill("#f-services-web-target", "4000").await;
        wait_checked(c).await;
        c.ui.click("[data-testid=save]").await;
        c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
        c.ui.shot("editing-web-target").await;
        let lab = super::world::secret();
        c.world.hand_edit(|t| {
            t.replacen(
                "clients:\n",
                &format!("clients:\n  lab:\n    secret: {lab}\n"),
                1,
            )
        });
        c.ui.wait_for("[data-testid=conflict]", Duration::from_secs(15))
            .await;
        c.ui.wait_text(
            "[data-testid=conflict-theirs]",
            "New client lab (1 secret)",
            SECS,
        )
        .await;
        c.ui.shot("conflict-dialog").await;
        c.ui.click("[data-testid=conflict] button[aria-label=Close]")
            .await;
        c.ui.wait_gone("[data-testid=conflict]", SECS).await;
        tokio::time::sleep(Duration::from_secs(6)).await;
        assert_eq!(
            c.ui.count("[data-testid=banner-changed]").await,
            1,
            "closing the dialog keeps the banner while the draft is on the old rev"
        );
        assert_eq!(
            c.ui.count("[data-testid=conflict]").await,
            0,
            "the closed dialog does not come back on its own"
        );
        c.ui.click("[data-testid=review-open]").await;
        c.ui.wait_for("[data-testid=review-stale]", SECS).await;
        c.ui.shot("review-on-the-old-rev").await;
        c.ui.click("[data-testid=review-stale] button").await;
        c.ui.wait_gone("[data-testid=review-stale]", SECS).await;
        c.ui.wait_for("[data-testid=review]", SECS).await;
        assert_eq!(
            c.ui.count("[data-testid=banner-changed]").await,
            0,
            "the banner goes with the draft it protected"
        );
        wait_checked(c).await;
        c.ui.shot("draft-kept-on-the-new-version").await;
        open_review(c, "review-after-recheck").await;
        applied(c, "applied-on-the-new-version").await;
        let text = c.world.config();
        assert!(text.contains("lab:"), "the hand edit stays:\n{text}");
        assert!(text.contains("target: 4000"), "my change is in:\n{text}");
        c.ui.assert_no_errors();
    })
}

#[test]
fn e5_a_hand_edit_during_editing_is_a_conflict_and_the_draft_is_kept() {
    run("e5-conflict", Options::default(), false, conflict);
}

async fn refused(c: &mut Ctx, code: &str, step: &str) {
    c.ui.wait_for(&format!("[data-refusal={code}]"), SECS).await;
    assert!(
        !c.ui.enabled("[data-testid=apply]").await,
        "no Apply for a lockout"
    );
    c.ui.wait_text("[data-testid=apply]", "Apply refused", SECS)
        .await;
    c.ui.shot(step).await;
}

fn lockout(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        let before = c.world.config();
        open(c, "ops").await;
        c.ui.nav("/visitors/ops").await;
        c.ui.shot("ops-visitor").await;
        edit_mode(c).await;
        c.ui.choose("f-visitors-ops-admin", UNSET).await;
        wait_checked(c).await;
        open_review(c, "removing-my-admin-right").await;
        refused(c, "self-demoted", "refused-self-demoted").await;
        c.ui.wait_for("[data-testid=fix-undo]", SECS).await;
        discard_from_review(c).await;

        c.ui.nav("/visitors/laptop").await;
        c.ui.nav("/visitors/ops").await;
        c.ui.click("[data-testid=more]").await;
        c.ui.click("[data-testid=delete]").await;
        wait_checked(c).await;
        c.ui.nav("/visitors/ops").await;
        c.ui.wait_for("[data-testid=undo-remove]", SECS).await;
        let gap: f64 = c
            .ui
            .eval("(() => { const a = document.querySelector('[data-testid=undo-remove]').getBoundingClientRect(); const b = document.querySelector('[data-testid=detail-missing] a').getBoundingClientRect(); return b.left >= a.right ? b.left - a.right : b.top - a.bottom; })()")
            .await;
        assert!(gap >= 6.0, "buttons side by side keep a gap: {gap}");
        c.ui.shot("ops-deleted-in-draft").await;
        open_review(c, "deleting-myself").await;
        refused(c, "self-removed", "refused-self-removed").await;
        discard_from_review(c).await;
        assert_eq!(c.world.config(), before, "server.yaml is unchanged");

        let url = c.world.open_admin("viewer");
        c.ui.goto("about:blank").await;
        c.ui.goto(&url).await;
        c.ui.wait_for("[data-testid=read-only-tag]", SECS).await;
        c.ui.nav("/services/web?tab=settings").await;
        c.ui.wait_for("[data-testid=detail]", SECS).await;
        c.ui.wait_text(
            "[data-schema='services/*/target']",
            &c.world.ports["echo"].to_string(),
            SECS,
        )
        .await;
        assert_eq!(
            c.ui.count("[data-editor]").await,
            0,
            "read access sees plain values, not controls that invite clicks"
        );
        assert_eq!(
            c.ui.count("[data-testid=save-bar]").await,
            0,
            "the read-only state is told once, by the header tag"
        );
        assert!(
            !c.ui.enabled("[data-testid=edit]").await,
            "Edit is disabled for read access"
        );
        let more: bool =
            c.ui.eval("document.querySelector('[data-testid=more]').offsetParent !== null")
                .await;
        assert!(!more, "read access gets no More menu");
        c.ui.nav("/services").await;
        assert!(
            !c.ui.enabled("[data-testid=new]").await,
            "+ is disabled for read access"
        );
        c.ui.shot("viewer-cannot-edit").await;
        c.ui.take_errors();
    })
}

#[test]
fn e7_the_lockout_guard_refuses_and_read_access_cannot_edit() {
    run("e7-lockout", Options::default(), false, lockout);
}

fn fallback_edit(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/services/capped").await;
        edit_mode(c).await;
        let row = "[data-schema='services/*/bandwidth']";
        c.ui.wait_text(row, "Checked by the server", SECS).await;
        c.ui.fill("#f-services-capped-bandwidth", "abc").await;
        wait_checked(c).await;
        c.ui.wait_text(&format!("{row} [aria-live]"), "abc", SECS)
            .await;
        c.ui.shot("server-rejects-abc").await;
        c.ui.fill("#f-services-capped-bandwidth", "50mbit").await;
        wait_checked(c).await;
        open_review(c, "review-50mbit").await;
        applied(c, "applied-50mbit").await;
        assert!(c.world.config().contains("bandwidth: 50mbit"));
        c.ui.assert_no_errors();
    })
}

#[test]
fn e8_a_field_of_a_newer_type_is_edited_as_text_and_checked_by_the_server() {
    let opts = Options {
        server_env: vec![("BURROW_ADMIN_TEST_SCHEMA".into(), "future".into())],
        extra_services: "  capped:\n    client: home\n    target: 8080\n    visitors: [laptop]\n    bandwidth: 40mbit\n".into(),
        ..Options::default()
    };
    run("e8-fallback-edit", opts, true, fallback_edit);
}

async fn activity_rev(c: &Ctx, index: usize) -> String {
    c.ui.nav("/activity").await;
    c.ui.wait_for("[data-testid=activity] [data-row]", SECS)
        .await;
    let revs =
        c.ui.all_attrs("[data-testid=activity] [data-row]", "data-row")
            .await;
    revs.get(index)
        .cloned()
        .unwrap_or_else(|| panic!("no entry {index} in {revs:?}"))
}

fn revert(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        let original = c.world.config();
        open(c, "ops").await;
        c.ui.nav("/services/web").await;
        edit_mode(c).await;
        c.ui.fill("#f-services-web-target", "4001").await;
        wait_checked(c).await;
        open_review(c, "change-to-revert").await;
        applied(c, "changed").await;
        let rev = activity_rev(c, 0).await;
        c.ui.shot("activity-with-the-change").await;
        c.ui.click(&format!("[data-row='{rev}'] [data-testid=list-row]"))
            .await;
        c.ui.wait_for("[data-testid=activity-diff]", SECS).await;
        c.ui.shot("diff-of-a-version").await;
        c.ui.click("[data-testid=revert]").await;
        c.ui.wait_for("[data-testid=review]", SECS).await;
        c.ui.shot("review-of-the-revert").await;
        applied(c, "reverted").await;
        assert_eq!(
            c.world.config(),
            original,
            "the revert restores the file byte for byte"
        );
        let at: String = c.ui.eval("location.pathname").await;
        if at.starts_with("/activity") {
            c.ui.wait_for("[data-testid=revert][disabled]", SECS).await;
        }
        c.ui.nav("/activity").await;
        c.ui.click(&format!("[data-row='{rev}'] [data-testid=list-row]"))
            .await;
        c.ui.wait_for("[data-testid=activity-diff]", SECS).await;
        assert!(
            !c.ui.enabled("[data-testid=revert]").await,
            "no revert when the file already matches that version"
        );
        c.ui.shot("a-revert-that-would-change-nothing").await;
        assert_eq!(c.world.config(), original);
        c.ui.assert_no_errors();
    })
}

#[test]
fn reverting_to_an_earlier_version_restores_the_file() {
    run("revert", Options::default(), false, revert);
}

fn revert_refused(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "boss").await;
        c.ui.nav("/visitors/ops").await;
        edit_mode(c).await;
        c.ui.choose("f-visitors-ops-admin", "read").await;
        wait_checked(c).await;
        open_review(c, "boss-demotes-ops").await;
        applied(c, "ops-is-read-only").await;
        c.ui.nav("/services/web").await;
        edit_mode(c).await;
        c.ui.fill("#f-services-web-target", "4002").await;
        wait_checked(c).await;
        open_review(c, "boss-changes-web").await;
        applied(c, "web-changed-while-ops-reads").await;

        let lab = super::world::secret();
        c.world.hand_edit(|t| {
            t.replacen(
                "# the admin\n    admin: read\n",
                "# the admin\n    admin: write\n",
                1,
            )
            .replacen(
                "clients:\n",
                &format!("clients:\n  lab:\n    secret: {lab}\n"),
                1,
            )
        });
        super::world::wait_until(
            "the hand edit that gives ops write again is running",
            Duration::from_secs(15),
            || {
                c.world
                    .status()
                    .and_then(|s| s["clients"].as_array().cloned())
                    .unwrap_or_default()
                    .iter()
                    .any(|x| x["name"] == "lab")
            },
        );

        let url = c.world.open_admin("ops");
        c.ui.goto("about:blank").await;
        c.ui.goto(&url).await;
        c.ui.wait_text("[data-testid=conn-pill]", "Connected", SECS)
            .await;
        let rev = activity_rev(c, 0).await;
        c.ui.click(&format!("[data-row='{rev}'] [data-testid=list-row]"))
            .await;
        c.ui.click("[data-testid=revert]").await;
        refused(c, "self-demoted", "revert-refused-by-the-guard").await;
        c.ui.take_errors();
    })
}

#[test]
fn a_revert_that_would_lock_me_out_is_refused() {
    run("revert-refused", Options::default(), false, revert_refused);
}

const IN_THE_FILE: &str = "Admin access is granted in the config file (burrow edit)";

async fn off(c: &Ctx, visitor: &str, level: &str) -> bool {
    c.ui.count(&format!(
        "#f-visitors-{visitor}-admin label[data-off=true] input[value='{level}']"
    ))
    .await
        == 1
}

fn grant_lock(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "boss").await;
        c.ui.nav("/visitors/viewer").await;
        edit_mode(c).await;
        c.ui.wait_text(
            "[data-schema='visitors/*/admin'] [data-testid=grant-lock]",
            IN_THE_FILE,
            SECS,
        )
        .await;
        assert!(off(c, "viewer", "write").await, "read cannot become write");
        assert!(!off(c, "viewer", "read").await);
        c.ui.choose("f-visitors-viewer-admin", "write").await;
        assert!(
            !c.ui.eval::<bool>("document.querySelector(\"#f-visitors-viewer-admin input[value='write']\").checked")
                .await,
            "a refused level is not chosen"
        );
        c.ui.shot("viewer-may-only-go-down").await;

        c.ui.nav("/visitors/plain").await;
        edit_mode(c).await;
        assert_eq!(
            c.ui.count("#f-visitors-plain-admin input").await,
            0,
            "no selector to offer when nothing may be chosen"
        );
        c.ui.wait_for(
            "[data-schema='visitors/*/admin'] [data-testid=grant-lock]",
            SECS,
        )
        .await;
        c.ui.shot("no-admin-for-a-visitor-without-it").await;

        c.ui.nav("/visitors/boss?tab=secrets").await;
        c.ui.wait_for("[data-testid=secrets] [data-testid=grant-lock]", SECS)
            .await;
        assert!(
            !c.ui.enabled("[data-testid=rotate]").await,
            "an admin gets a new secret only in the file"
        );
        c.ui.shot("admin-secret-locked").await;
        c.ui.click("[data-testid=more]").await;
        c.ui.wait_for("[data-testid=more-menu] [data-testid=grant-lock]", SECS)
            .await;
        assert!(
            !c.ui.enabled("[data-testid=rename-open]").await,
            "an admin visitor is renamed only in the file"
        );
        c.ui.shot("admin-rename-locked").await;
        c.ui.click("[data-testid=more]").await;
        c.ui.nav("/visitors/plain").await;
        c.ui.click("[data-testid=more]").await;
        c.ui.wait_for("[data-testid=more-menu] [data-testid=rename-open]", SECS)
            .await;
        assert!(c.ui.enabled("[data-testid=rename-open]").await);
        assert_eq!(
            c.ui.count("[data-testid=more-menu] [data-testid=grant-lock]")
                .await,
            0
        );
        c.ui.click("[data-testid=more]").await;

        c.ui.nav("/visitors/ops").await;
        edit_mode(c).await;
        let from = "#f-visitors-ops-admin-from";
        let chips = "[data-schema='visitors/*/admin_from'] [data-chip]";
        c.ui.click("[data-schema='visitors/*/admin_from'] [data-chip='127.0.0.1/32'] button")
            .await;
        c.ui.wait_text(
            "[data-schema='visitors/*/admin_from'] [data-testid=grant-lock]",
            IN_THE_FILE,
            SECS,
        )
        .await;
        assert_eq!(c.ui.count(chips).await, 1, "the last entry stays");
        c.ui.fill(from, "10.0.0.0/8").await;
        c.ui.wait_for(
            "[data-schema='visitors/*/admin_from'] [data-testid=grant-lock]",
            SECS,
        )
        .await;
        assert_eq!(c.ui.count(chips).await, 1, "a wider entry is not added");
        c.ui.shot("admin-from-may-not-widen").await;
        c.ui.fill(from, "127.0.0.1").await;
        c.ui.wait_for(
            "[data-schema='visitors/*/admin_from'] [data-chip='127.0.0.1']",
            SECS,
        )
        .await;
        c.ui.wait_gone(
            "[data-schema='visitors/*/admin_from'] [data-testid=grant-lock]",
            SECS,
        )
        .await;
        wait_checked(c).await;
        c.ui.click("[data-testid=save]").await;
        c.ui.wait_gone("[data-testid=save-bar]", SECS).await;

        c.ui.nav("/visitors/viewer").await;
        edit_mode(c).await;
        c.ui.choose("f-visitors-viewer-admin", UNSET).await;
        wait_checked(c).await;
        open_review(c, "viewer-loses-admin").await;
        applied(c, "viewer-is-no-admin").await;
        let text = c.world.config();
        assert!(!text.contains("admin: read"), "{text}");
        assert!(
            text.contains("admin_from: [127.0.0.1/32, 127.0.0.1]"),
            "{text}"
        );
        c.ui.assert_no_errors();
    })
}

#[test]
fn admin_power_is_only_lowered_in_the_visitor_form() {
    run("grant-lock", Options::default(), false, grant_lock);
}

fn revert_conflict(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/services/web").await;
        edit_mode(c).await;
        c.ui.fill("#f-services-web-target", "4002").await;
        wait_checked(c).await;
        open_review(c, "a-change").await;
        applied(c, "changed").await;
        let rev = activity_rev(c, 0).await;
        c.ui.click(&format!("[data-row='{rev}'] [data-testid=list-row]"))
            .await;
        c.ui.click("[data-testid=revert]").await;
        c.ui.wait_for("[data-testid=apply]:not([disabled])", SECS)
            .await;
        c.ui.shot("revert-reviewed").await;
        c.world
            .hand_edit(|t| t.replace("target: 4002", "target: 4003"));
        c.ui.wait_for("[data-testid=banner-changed]", SECS).await;
        c.ui.click("[data-testid=apply]").await;
        c.ui.wait_for("[data-testid=conflict]", SECS).await;
        c.ui.shot("revert-against-a-changed-file").await;
        c.ui.click("[data-testid=conflict-recheck]").await;
        c.ui.wait_for("[data-testid=apply]:not([disabled])", SECS)
            .await;
        c.ui.shot("revert-reviewed-again-on-the-new-version").await;
        assert!(
            c.world.config().contains("target: 4003"),
            "the hand edit is untouched"
        );
        c.ui.take_errors();
    })
}

#[test]
fn a_revert_against_a_changed_file_is_a_conflict() {
    run(
        "revert-conflict",
        Options::default(),
        false,
        revert_conflict,
    );
}

fn typed_confirm(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        let echo = echo_server(c.world.ports["echo"]).await;
        let port = c.world.ports["web"];
        let held = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect");
        open(c, "ops").await;
        c.ui.nav("/services/web").await;
        c.ui.wait_not_text("[data-testid=live] [data-stat=open]", "0", SECS)
            .await;
        c.ui.shot("web-with-an-open-connection").await;
        c.ui.click("[data-testid=more]").await;
        c.ui.click("[data-testid=delete]").await;
        wait_checked(c).await;
        open_review(c, "impact-of-removing-web").await;
        let impacts = c.ui.text("[data-testid=review]").await;
        assert!(impacts.contains("open connection"), "{impacts}");
        c.ui.wait_for("[data-impact][data-level=danger]", SECS)
            .await;
        c.ui.wait_for("[data-testid=confirms] [data-confirm=web]", SECS)
            .await;
        assert!(
            !c.ui.enabled("[data-testid=apply]").await,
            "Apply waits for the typed name"
        );
        c.ui.fill("[data-confirm=web]", "we").await;
        assert!(!c.ui.enabled("[data-testid=apply]").await);
        c.ui.shot("typed-confirmation-incomplete").await;
        c.ui.fill("[data-confirm=web]", "web").await;
        c.ui.shot("typed-confirmation-done").await;
        applied(c, "web-removed").await;
        assert!(!c.world.config().contains("  web:"));
        drop(held);
        echo.abort();
        c.ui.assert_no_errors();
    })
}

#[test]
fn removing_a_busy_service_shows_its_impact_and_needs_its_name_typed() {
    run("typed-confirm", Options::default(), false, typed_confirm);
}

fn locked_fields(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/settings").await;
        c.ui.wait_for("[data-schema=listen]", SECS).await;
        for field in [
            "listen",
            "insecure_plaintext",
            "heartbeat/interval_secs",
            "limits/handshake_timeout_secs",
            "limits/max_handshakes",
        ] {
            let row = format!("[data-schema='{field}']");
            assert_eq!(
                c.ui.count(&format!("{row} [data-editor]")).await,
                0,
                "{field} has no edit control"
            );
            assert_eq!(
                c.ui.count(&format!("{row} input, {row} select")).await,
                0,
                "{field} has no input"
            );
            let text =
                c.ui.wait_text(&row, "Change this in server.yaml (burrow edit)", SECS)
                    .await;
            assert_eq!(
                text.contains("then restart"),
                field == "listen",
                "{field}: only listen needs a restart: {text}"
            );
            assert_eq!(
                text.matches("Change this").count(),
                1,
                "{field} says it once"
            );
        }
        assert_eq!(
            c.ui.count("[data-schema=websocket] input").await,
            0,
            "the websocket block has no controls"
        );
        c.ui.wait_for("[data-schema=websocket] [data-testid=locked-hint]", SECS)
            .await;
        assert_eq!(
            c.ui.count("[data-schema=public_address] input").await,
            0,
            "view mode shows values"
        );
        edit_mode(c).await;
        assert!(
            c.ui.count("[data-schema=public_address] input").await > 0,
            "unlocked fields are editable in edit mode"
        );
        assert!(
            c.ui.count("[data-schema='limits/max_pending'] input").await > 0,
            "the rest of limits stays editable"
        );
        c.ui.shot("locked-fields-are-read-only").await;
        assert_eq!(c.ui.count("[data-testid=review-open]").await, 0);
        c.ui.assert_no_errors();
    })
}

#[test]
fn locked_server_fields_are_read_only() {
    run("locked-fields", Options::default(), false, locked_fields);
}

async fn revert_review_left(c: &mut Ctx, ready: bool) {
    let rev = activity_rev(c, 0).await;
    c.ui.click(&format!("[data-row='{rev}'] [data-testid=list-row]"))
        .await;
    c.ui.wait_for("[data-testid=activity-diff]", SECS).await;
    c.ui.click("[data-testid=revert]").await;
    c.ui.wait_for("[data-testid=review]", SECS).await;
    if ready {
        c.ui.wait_for("[data-testid=apply]:not([disabled])", SECS)
            .await;
    }
}

fn stale_revert(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/services/web").await;
        edit_mode(c).await;
        c.ui.fill("#f-services-web-target", "4001").await;
        wait_checked(c).await;
        open_review(c, "change-to-revert").await;
        applied(c, "changed").await;
        revert_review_left(c, true).await;
        c.ui.shot("revert-reviewed-then-left").await;
        c.ui.nav("/clients").await;
        c.ui.click("[data-testid=new]").await;
        c.ui.fill("#new-name", "lab9").await;
        c.ui.click("[data-testid=create-apply]").await;
        c.ui.wait_for("[data-testid=shown-once]", Duration::from_secs(20))
            .await;
        c.ui.shot("create-and-apply-after-a-left-revert").await;
        let after = c.world.config();
        assert!(
            after.contains("lab9:"),
            "the new client is applied: {after}"
        );
        assert!(
            after.contains("4001"),
            "the revert was applied without a click: {after}"
        );
        c.ui.click("[data-testid=saved]").await;
        c.ui.click("[data-testid=close-shown-once]").await;
        c.ui.wait_gone("[data-testid=shown-once]", SECS).await;

        revert_review_left(c, false).await;
        c.ui.nav("/services/web").await;
        edit_mode(c).await;
        c.ui.fill("#f-services-web-target", "4002").await;
        wait_checked(c).await;
        open_review(c, "draft-review-after-a-left-revert").await;
        c.ui.wait_for("[data-testid=apply]:not([disabled])", SECS)
            .await;
        let label = c.ui.text("[data-testid=apply]").await;
        assert!(
            !label.contains("revert"),
            "the draft's review shows: {label}"
        );
        c.ui.take_errors();
    })
}

#[test]
fn a_revert_review_left_unapplied_is_never_applied_later() {
    run("stale-revert", Options::default(), false, stale_revert);
}

fn revert_after_restart(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        let original = c.world.config();
        open(c, "ops").await;
        c.ui.nav("/services/web").await;
        edit_mode(c).await;
        c.ui.fill("#f-services-web-target", "4001").await;
        wait_checked(c).await;
        open_review(c, "change-before-a-restart").await;
        applied(c, "changed").await;
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
        let older = activity_rev(c, 1).await;
        c.ui.click(&format!("[data-row='{older}'] [data-testid=list-row]"))
            .await;
        c.ui.wait_for("[data-testid=revert]", SECS).await;
        c.ui.shot("a-kept-version-after-a-restart").await;
        c.ui.click("[data-testid=revert]").await;
        c.ui.wait_for("[data-testid=review]", SECS).await;
        c.ui.shot("review-of-a-kept-version").await;
        applied(c, "reverted-after-a-restart").await;
        assert_eq!(
            c.world.config(),
            original,
            "the revert brings back the kept version"
        );
        c.ui.take_errors();
    })
}

#[test]
fn a_kept_version_can_be_reverted_after_a_server_restart() {
    run(
        "revert-after-restart",
        Options::default(),
        false,
        revert_after_restart,
    );
}

fn draft_after_a_viewed_revert_conflict(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/services/web").await;
        edit_mode(c).await;
        c.ui.fill("#f-services-web-target", "4002").await;
        wait_checked(c).await;
        open_review(c, "a-change").await;
        applied(c, "changed").await;
        revert_review_left(c, true).await;
        c.world
            .hand_edit(|t| t.replace("target: 4002", "target: 4003"));
        c.ui.wait_for("[data-testid=banner-changed]", SECS).await;
        c.ui.click("[data-testid=apply]").await;
        c.ui.wait_for("[data-testid=conflict]", SECS).await;
        c.ui.click("[data-testid=conflict-view]").await;
        c.ui.wait_gone("[data-testid=conflict]", SECS).await;
        c.ui.nav("/services/web").await;
        edit_mode(c).await;
        c.ui.fill("#f-services-web-target", "4005").await;
        wait_checked(c).await;
        c.world.hand_edit(|t| format!("{t}\n# touched\n"));
        c.ui.wait_for("[data-testid=conflict]", Duration::from_secs(15))
            .await;
        c.ui.click("[data-testid=conflict-recheck]").await;
        c.ui.wait_gone("[data-testid=conflict]", SECS).await;
        wait_checked(c).await;
        open_review(c, "review-my-draft-after-viewing-a-revert-conflict").await;
        c.ui.wait_for("[data-testid=apply]", SECS).await;
        let label = c.ui.text("[data-testid=apply]").await;
        assert!(
            !label.contains("revert"),
            "the draft's review opens: {label}"
        );
        let page = c.ui.text("[data-testid=review]").await;
        assert!(page.contains("4005"), "the draft is reviewed: {page}");
        c.ui.take_errors();
    })
}

#[test]
fn review_my_draft_opens_the_draft_after_a_revert_conflict_was_viewed() {
    run(
        "draft-after-revert-conflict",
        Options::default(),
        false,
        draft_after_a_viewed_revert_conflict,
    );
}

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
        c.ui.nav("/clients/home").await;
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
        c.ui.assert_no_errors();
    })
}

#[test]
fn edits_survive_navigation_and_problems_show_where_you_edit() {
    run("edit-state", Options::default(), false, edit_state);
}
