use std::path::PathBuf;
use std::time::Duration;

use super::releases::Releases;
use super::world::{BIN, Options};
use super::{Ctx, SECS, both_widths, open, overview_tab, run};

static URL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static LAB: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static SPARE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

const OLD: &str = "0.0.1-test.1";
const NEW: &str = "0.0.1-test.2";

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
        assert!(home.ends_with(" · by hand"), "{home}");
        let office = c.ui.text(&row("office")).await;
        assert!(office.ends_with(" · by hand · updates off"), "{office}");
        assert!(
            c.ui.attr_of(&row("office"), "title")
                .await
                .ends_with("remote updates off in its config")
        );
        let all = c.ui.text("[data-testid=update-all]").await;
        assert_eq!(
            all,
            format!("Update all to {NEW} (2)"),
            "only home and spare can take it"
        );
        every_look(c, "clients-versions").await;

        c.ui.nav("/clients/office").await;
        c.ui.wait_for("[data-testid=live] [data-testid=client-facts]", SECS)
            .await;
        assert_eq!(
            c.ui.count("[data-testid=client-burrow]").await,
            0,
            "the client's burrow sits in its overview figures, not in a card of its own"
        );
        assert!(
            !c.ui.enabled("[data-testid=client-update]").await,
            "office did not opt in"
        );
        let why =
            c.ui.eval::<String>(
                "document.querySelector('[data-testid=client-update]').parentElement.title",
            )
            .await;
        assert!(
            why.contains("off in its config"),
            "the reason shows on hover: {why}"
        );
        let remote = c.ui.text("[data-testid=client-facts]").await;
        assert!(remote.contains("off in its config"), "{remote}");
        every_look(c, "client-not-allowed").await;

        c.ui.nav("/clients/lab").await;
        c.ui.wait_for("[data-testid=client-older]", SECS).await;
        let older = c.ui.text("[data-testid=client-older]").await;
        assert!(older.contains("sudo burrow update"), "{older}");
        every_look(c, "client-older").await;

        c.ui.nav("/clients/home").await;
        c.ui.wait_for("[data-testid=client-update]", SECS).await;
        every_look(c, "client-update-offer").await;
        c.ui.click("[data-testid=client-update]").await;
        c.ui.wait_for("[data-testid=client-update-dialog]", SECS)
            .await;
        every_look(c, "client-update-dialog").await;
        c.ui.click("[data-testid=client-update-confirm]").await;
        c.ui.wait_text(
            "[data-testid=client-last-update]",
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
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_client_is_updated_from_its_page_and_others_say_why_not() {
    let releases = Releases::start();
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
            server_env: vec![("BURROW_UPDATE_URL".into(), base)],
            ..Options::default()
        },
        true,
        flow,
    );
    drop(releases);
}
