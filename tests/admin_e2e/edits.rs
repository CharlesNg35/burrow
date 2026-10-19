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
        c.ui.wait_text(
            "[data-schema='services/*/bind'] [data-testid=stored-as]",
            &format!("127.0.0.1:{}-{}", public, public + 2),
            SECS,
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
        c.ui.wait_for("[data-testid=review-open][data-state=errors]", SECS)
            .await;
        assert!(
            !c.ui.enabled("[data-testid=save]").await,
            "a server error blocks saving"
        );
        c.ui.shot("server-says-duplicate-bind").await;
        c.ui.click("[data-schema='services/*/bind'] [data-testid=reset-field]")
            .await;
        c.ui.wait_gone("[data-testid=review-open]", SECS).await;
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

async fn row_error(c: &Ctx, schema: &str) -> String {
    let css = format!("[data-schema='{schema}'] [aria-live] p");
    c.ui.wait_for(&css, SECS).await;
    c.ui.text(&css).await
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
        c.ui.wait_text(
            "[data-schema='services/*/target'] [data-testid=stored-as]",
            "unix:/tmp/burrow-e2e.sock",
            SECS,
        )
        .await;
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
        c.ui.wait_for("[data-server-address]", SECS).await;
        click_js(c, "[data-server-address] input[value=host]").await;
        c.ui.fill("#f-public-address", "bad host").await;
        let err = row_error(c, "public_address").await;
        assert!(!err.is_empty(), "a host with a space is refused inline");
        c.ui.shot("server-address-invalid").await;
        click_js(c, "[data-server-address] input[value=wss]").await;
        c.ui.fill("#f-public-address", "tunnel.example.net/burrow")
            .await;
        row_clean(c, "public_address").await;
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
        c.ui.shot("conflict-dialog").await;
        c.ui.click("[data-testid=conflict-recheck]").await;
        c.ui.wait_gone("[data-testid=conflict]", SECS).await;
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
        c.ui.shot("ops-deleted-in-draft").await;
        open_review(c, "deleting-myself").await;
        refused(c, "self-removed", "refused-self-removed").await;
        discard_from_review(c).await;
        assert_eq!(c.world.config(), before, "server.yaml is unchanged");

        let url = c.world.open_admin("viewer");
        c.ui.goto("about:blank").await;
        c.ui.goto(&url).await;
        c.ui.wait_text("[data-testid=conn-pill]", "Read-only", SECS)
            .await;
        c.ui.nav("/services/web?tab=settings").await;
        c.ui.wait_for("[data-testid=detail]", SECS).await;
        assert!(
            c.ui.count("[data-editor]").await > 0,
            "read access sees the controls in their read-only look"
        );
        assert_eq!(
            c.ui.count("[data-editor]:not([inert])").await,
            0,
            "every control is inert for read access"
        );
        c.ui.wait_text(
            "[data-testid=save-bar]",
            "Read-only: changes need write admin",
            SECS,
        )
        .await;
        assert!(
            !c.ui.enabled("[data-testid=edit]").await,
            "Edit is disabled for read access"
        );
        c.ui.click("[data-testid=more]").await;
        assert_eq!(c.ui.count("[data-testid=delete]").await, 0);
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
        c.ui.nav("/visitors/laptop").await;
        c.ui.nav("/visitors/ops").await;
        edit_mode(c).await;
        c.ui.choose("f-visitors-ops-admin", "write").await;
        wait_checked(c).await;
        open_review(c, "boss-promotes-ops").await;
        applied(c, "ops-can-write-again").await;

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
            c.ui.wait_text(
                &row,
                "Change this in server.yaml on the server, then restart",
                SECS,
            )
            .await;
        }
        assert_eq!(
            c.ui.count("[data-schema=websocket] input").await,
            0,
            "the websocket block has no controls"
        );
        c.ui.wait_for("[data-schema=websocket] [data-testid=locked-hint]", SECS)
            .await;
        assert!(
            c.ui.count("[data-schema=public_address] input").await > 0,
            "unlocked fields stay editable"
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
