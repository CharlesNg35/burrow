use std::path::PathBuf;
use std::time::Duration;

use super::releases::Releases;
use super::world::{BIN, Options};
use super::{Ctx, SECS, both_widths, open, overview_tab, run};

static URL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static RELEASES: std::sync::OnceLock<Releases> = std::sync::OnceLock::new();
static LAB: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static SPARE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

const OLD: &str = "0.0.1-test.1";
const NEW: &str = "0.0.1-test.2";
const BROKEN: &str = "0.0.1-test.3";

fn marked(marks: &[&str]) -> Vec<u8> {
    let mut b = std::fs::read(BIN).unwrap();
    b.push(b'\n');
    for m in marks {
        b.extend_from_slice(format!("BURROW-TEST-{m}\n").as_bytes());
    }
    b
}

fn binary(c: &Ctx, name: &str, marks: &[&str]) -> PathBuf {
    let dir = c.world.file(&format!("bin-{name}"));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(if cfg!(windows) {
        "burrow.exe"
    } else {
        "burrow"
    });
    std::fs::write(&path, marked(marks)).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

fn start(c: &mut Ctx, name: &str, marks: &[&str], extra: &str) {
    let url = URL.get().cloned().unwrap_or_default();
    let url = url.as_str();
    let bin = binary(c, name, marks);
    let secret = match name {
        "lab" => LAB.get().cloned().unwrap_or_default(),
        "spare" => SPARE.get().cloned().unwrap_or_default(),
        _ => c.world.secrets[name].clone(),
    };
    let yaml = format!(
        "server: {}\nname: {name}\nsecret: {secret}\nallow: [127.0.0.1/32]\n{extra}",
        c.world.server_addr(),
    );
    let file = format!("{name}.yaml");
    c.world.write(&file, &yaml);
    c.world.start_bin(
        &bin,
        name,
        &["client", "-c", &file],
        &[
            ("BURROW_UPDATE_URL", url),
            ("BURROW_ADMIN_TEST_UPDATE_WINDOW", "5"),
        ],
    );
    c.world.wait_client(name, true);
}

async fn every_look(c: &mut Ctx, step: &str) {
    both_widths(c, step).await;
    c.ui.viewport(1280, 900).await;
    c.ui.shot(&format!("{step}-1280")).await;
    c.ui.viewport(390, 900).await;
    c.ui.shot(&format!("{step}-390")).await;
    c.ui.viewport(1440, 900).await;
}

pub async fn whats_new(c: &Ctx, dialog: &str, version: &str) {
    let link = format!("[data-testid={dialog}] [data-testid=whats-new]");
    assert_eq!(c.ui.text(&link).await, format!("What's new in {version}"));
    assert_eq!(
        c.ui.attr_of(&link, "href").await,
        format!("https://github.com/CharlesNg35/burrow/releases/tag/v{version}")
    );
    assert_eq!(c.ui.attr_of(&link, "target").await, "_blank");
    assert_eq!(c.ui.attr_of(&link, "rel").await, "noopener noreferrer");
    let fetched: f64 = c
        .ui
        .eval("performance.getEntriesByType('resource').filter(e => e.name.includes('github')).length")
        .await;
    assert_eq!(fetched, 0.0, "the page fetches nothing from GitHub");
}

fn flow(c: &mut Ctx) -> super::edits::Flow<'_> {
    Box::pin(async move {
        start(
            c,
            "home",
            &[&format!("VERSION={OLD}")],
            "update:\n  remote: true\n",
        );
        start(c, "office", &[&format!("VERSION={OLD}")], "");
        start(c, "lab", &["REPORT=none"], "");
        start(
            c,
            "spare",
            &[&format!("VERSION={OLD}")],
            "update:\n  remote: true\n",
        );
        open(c, "ops").await;
        c.ui.wait_for("[data-testid=overview-sub]", SECS).await;
        assert_eq!(
            c.ui.count("[data-testid=server-burrow]").await,
            0,
            "no newer release for the server: the header says nothing more"
        );
        let sub = c.ui.text("[data-testid=overview-sub]").await;
        assert!(sub.starts_with('v') && !sub.contains("available"), "{sub}");
        overview_tab(c, "clients").await;
        c.ui.wait_text(
            "[data-testid=overview-clients] tr[data-row=home] [data-testid=client-update-tag]",
            &format!("{NEW} available"),
            Duration::from_secs(20),
        )
        .await;
        let lab = c
            .ui
            .text("[data-testid=overview-clients] tr[data-row=lab] [data-testid=client-version]")
            .await;
        assert_eq!(lab, "older than 0.2.2");
        let row = |n: &str| {
            format!("[data-testid=overview-clients] tr[data-row={n}] [data-testid=client-system]")
        };
        let home = c.ui.text(&row("home")).await;
        assert!(
            home.contains('/') && !home.contains('·'),
            "the system only: {home}"
        );
        let office_tag =
            "[data-testid=overview-clients] tr[data-row=office] [data-testid=client-update-tag]";
        assert_eq!(c.ui.text(office_tag).await, format!("{NEW} available"));
        assert!(
            c.ui.attr_of(office_tag, "title")
                .await
                .contains("remote updates are off in its config"),
            "updates off is the tag's tooltip"
        );
        let all = c.ui.text("[data-testid=update-all]").await;
        assert_eq!(
            all,
            format!("Update all to {NEW} (2)"),
            "only home and spare can take it"
        );
        every_look(c, "clients-versions").await;

        c.ui.nav("/clients/office").await;
        let tag = "[data-testid=live] [data-testid=client-update-tag]";
        c.ui.wait_text(tag, &format!("{NEW} available"), SECS).await;
        assert_eq!(
            c.ui.count("[data-testid=client-update]").await,
            0,
            "office did not opt in: no button, not a disabled one"
        );
        let why = c.ui.attr_of(tag, "title").await;
        assert!(
            why.contains("off in its config"),
            "the reason is the tag's tooltip: {why}"
        );
        assert_eq!(
            c.ui.text("[data-testid=live] [data-stat=burrow]").await,
            OLD
        );
        every_look(c, "client-not-allowed").await;

        c.ui.nav("/clients/lab").await;
        c.ui.wait_for("[data-testid=live] [data-stat=burrow]", SECS)
            .await;
        let live = c.ui.text("[data-testid=live]").await;
        assert!(live.contains("older than 0.2.2"), "{live}");
        let hint =
            c.ui.eval::<String>(
                "document.querySelector('[data-testid=live] [data-stat=burrow]').closest('[title]')?.title ?? ''",
            )
            .await;
        assert!(
            hint.contains("sudo burrow update"),
            "the hint is a tooltip: {hint}"
        );
        every_look(c, "client-older").await;

        c.ui.nav("/clients/home").await;
        c.ui.wait_for("[data-testid=client-update]", SECS).await;
        every_look(c, "client-update-offer").await;
        c.ui.click("[data-testid=client-update]").await;
        c.ui.wait_for("[data-testid=client-update-dialog]", SECS)
            .await;
        whats_new(c, "client-update-dialog", NEW).await;
        every_look(c, "client-update-dialog").await;
        c.ui.click("[data-testid=client-update-confirm]").await;
        c.ui.wait_text(
            "[data-testid=live] [data-testid=client-update-tag]",
            "updated",
            Duration::from_secs(60),
        )
        .await;
        let shown = c.ui.text("[data-testid=live] [data-stat=burrow]").await;
        assert!(shown.contains(NEW), "{shown}");
        assert_eq!(
            c.ui.count("[data-testid=client-update]").await,
            0,
            "nothing left to update"
        );
        every_look(c, "client-updated").await;

        c.ui.nav("/").await;
        overview_tab(c, "clients").await;
        c.ui.wait_text(
            "[data-testid=update-all]",
            &format!("Update all to {NEW} (1)"),
            SECS,
        )
        .await;
        c.ui.click("[data-testid=update-all]").await;
        c.ui.wait_for("[data-testid=update-all-dialog]", SECS).await;
        whats_new(c, "update-all-dialog", NEW).await;
        both_widths(c, "update-all-dialog").await;
        c.ui.click("[data-testid=update-all-confirm]").await;
        c.ui.wait_text(
            "[data-testid=rollout]",
            "Updated spare",
            Duration::from_secs(60),
        )
        .await;
        assert_eq!(c.ui.count("[data-testid=update-all]").await, 0);
        every_look(c, "update-all-done").await;
        rolled_back(c).await;
        c.ui.assert_no_errors();
    })
}

async fn rolled_back(c: &mut Ctx) {
    let releases = RELEASES.get().expect("the release server");
    releases.publish(
        BROKEN,
        &marked(&[&format!("VERSION={BROKEN}"), "BREAK=connect"]),
        false,
    );
    c.ui.nav("/clients/home").await;
    let tag = "[data-testid=live] [data-testid=client-update-tag]";
    c.ui.wait_text(tag, &format!("{BROKEN} available"), Duration::from_secs(30))
        .await;
    c.ui.click("[data-testid=client-update]").await;
    c.ui.wait_for("[data-testid=client-update-dialog]", SECS)
        .await;
    c.ui.click("[data-testid=client-update-confirm]").await;
    c.ui.wait_text(tag, "updating…", SECS).await;
    every_look(c, "client-updating").await;
    c.ui.wait_text(tag, "rolled back", Duration::from_secs(60))
        .await;
    let why = c.ui.attr_of(tag, "title").await;
    assert!(
        why.contains("rolled back: "),
        "the reason is the tooltip: {why}"
    );
    assert_eq!(
        c.ui.count("[data-testid=client-update]").await,
        1,
        "it may be asked again"
    );
    every_look(c, "client-rolled-back").await;
}

#[test]
fn a_client_is_updated_from_its_page_and_others_say_why_not() {
    let releases = RELEASES.get_or_init(Releases::start);
    releases.publish(NEW, &marked(&[&format!("VERSION={NEW}")]), false);
    let base = releases.base.clone();
    let lab = LAB.get_or_init(super::world::secret).clone();
    let spare = SPARE.get_or_init(super::world::secret).clone();
    URL.get_or_init(|| base.clone());
    run(
        "client-updates",
        Options {
            clients: Some(Vec::new()),
            extra_clients: format!("  lab:\n    secret: {lab}\n  spare:\n    secret: {spare}\n"),
            server_env: vec![
                ("BURROW_UPDATE_URL".into(), base),
                ("BURROW_ADMIN_TEST_LATEST_SECS".into(), "1".into()),
            ],
            ..Options::default()
        },
        true,
        flow,
    );
}
