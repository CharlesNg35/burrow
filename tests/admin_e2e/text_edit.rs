use std::time::Duration;

use super::edits::Flow;
use super::world::{self, Options};
use super::{Ctx, SECS, apply_now, open, run};

const CARD: &str = "[data-testid=config-file]";
const EDITOR: &str = "[data-testid=file-editor]";
const REVIEW: &str = "[data-testid=file-review]";
const MAIN: &str = "server.yaml";
const APPS: &str = "server.d/apps.yaml";
const TOOL: &str = "server.d/tool.yaml";
const CHECKED: Duration = Duration::from_secs(15);

fn opts() -> Options {
    Options {
        drop_ins: vec![
            (
                "apps.yaml",
                "# apps, kept by hand\nservices:\n  blog:\n    client: home # the home box\n    target: 22\n    visitors: [laptop]\n".into(),
                false,
            ),
            (
                "tool.yaml",
                format!(
                    "# written by a tool\nclients:\n  box:\n    secret: {}\n",
                    world::secret()
                ),
                true,
            ),
        ],
        extra_top: "web:\n  http: 127.0.0.1:HTTP_PORT\n".into(),
        extra_services: format!(
            "  site:\n    client: home\n    target: 8080\n    web:\n      hosts: [site.example.com]\n      tls: \"off\"\n      request_headers: {{ Authorization: \"{HEADER}\" }}\n"
        ),
        ..Options::default()
    }
}

const HEADER: &str = "Bearer e2e-text-header-5b1e";
const PILL: &str = "[data-testid=conn-pill]";

fn js(s: &str) -> String {
    serde_json::to_string(s).expect("a JS string")
}

fn tab(name: &str) -> String {
    format!("{EDITOR} [role=tab][data-document-id='{name}']")
}

fn input(name: &str) -> String {
    format!("{EDITOR} [role=tabpanel][data-document-id='{name}'] .config-editor__input")
}

async fn start(c: &Ctx) {
    c.ui.nav("/settings?tab=file").await;
    c.ui.click("[data-testid=file-edit]").await;
    c.ui.wait_for(&format!("{CARD}[data-editing=true]"), SECS)
        .await;
    c.ui.wait_for(&input(MAIN), SECS).await;
}

async fn show(c: &Ctx, name: &str) {
    c.ui.click(&tab(name)).await;
    c.ui.wait_for(&format!("{}[aria-selected=true]", tab(name)), SECS)
        .await;
    c.ui.wait_for(&input(name), SECS).await;
}

async fn text_of(c: &Ctx, name: &str) -> String {
    c.ui.eval(&format!(
        "document.querySelector({}).value",
        js(&input(name))
    ))
    .await
}

async fn replace_in(c: &Ctx, name: &str, from: &str, to: &str) {
    let ok: bool = c
        .ui
        .eval(&format!(
            "(() => {{ const i = document.querySelector({sel}); const from = {from}, to = {to}; const at = i ? i.value.indexOf(from) : -1; if (at < 0) return false; i.focus(); i.setSelectionRange(at, at + from.length); i.setRangeText(to, at, at + from.length, 'end'); i.dispatchEvent(new InputEvent('input', {{ bubbles: true, inputType: 'insertFromPaste', data: to }})); return true; }})()",
            sel = js(&input(name)),
            from = js(from),
            to = js(to)
        ))
        .await;
    assert!(ok, "{name} has {from:?} to replace");
}

async fn dirty(c: &Ctx, name: &str, on: bool) {
    c.ui.wait_for(&format!("{}[data-dirty={on}]", tab(name)), SECS)
        .await;
}

async fn save_key(c: &Ctx, name: &str) {
    let _: bool = c
        .ui
        .eval(&format!(
            "(() => {{ const i = document.querySelector({}); i.focus(); i.dispatchEvent(new KeyboardEvent('keydown', {{ key: 's', code: 'KeyS', ctrlKey: true, bubbles: true, cancelable: true }})); return true; }})()",
            js(&input(name))
        ))
        .await;
}

async fn burrow_problem(c: &Ctx, name: &str, words: &str) {
    let panel = js(&format!(
        "{EDITOR} [role=tabpanel][data-document-id='{name}']"
    ));
    let found = format!(
        "(() => {{ const p = document.querySelector({panel}); const w = {}; return !!p && [...p.querySelectorAll('.config-editor__diagnostic-marker-host')].some(e => (e.dataset.message || '').includes(w)); }})()",
        js(words)
    );
    let started = std::time::Instant::now();
    while started.elapsed() < CHECKED {
        if c.ui.eval::<bool>(&found).await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let seen: Vec<String> = c
        .ui
        .eval(&format!(
            "(() => {{ const p = document.querySelector({panel}); if (!p) return ['no panel']; return [...p.querySelectorAll('.config-editor__diagnostic-marker')].map(e => (e.classList.contains('config-editor__diagnostic-marker-host') ? 'host: ' : 'editor: ') + (e.dataset.message || '')).concat([...p.querySelectorAll('.config-editor__footer, footer')].map(f => 'footer: ' + f.textContent)); }})()"
        ))
        .await;
    panic!(
        "burrow's problem in {name} does not say {words:?} within {CHECKED:?}; the editor shows {seen:#?}"
    );
}

const WATCH_CSP: &str = "(() => { if (!window.__csp) { window.__csp = []; document.addEventListener('securitypolicyviolation', e => window.__csp.push(e.violatedDirective + ' ' + (e.sample || e.blockedURI) + ' at ' + (e.sourceFile || '') + ':' + e.lineNumber), true); } return true; })()";

async fn key(c: &Ctx, name: &str, key: &str, code: &str, mods: &str) {
    let _: bool = c
        .ui
        .eval(&format!(
            "(() => {{ const i = document.querySelector({}); i.focus(); i.dispatchEvent(new KeyboardEvent('keydown', {{ key: {}, code: {}, {mods} bubbles: true, cancelable: true }})); return true; }})()",
            js(&input(name)),
            js(key),
            js(code)
        ))
        .await;
    tokio::time::sleep(Duration::from_millis(250)).await;
}

async fn caret_on(c: &Ctx, name: &str, word: &str) {
    let ok: bool = c
        .ui
        .eval(&format!(
            "(() => {{ const i = document.querySelector({}); const at = i.value.indexOf({}); if (at < 0) return false; i.focus(); i.setSelectionRange(at + 1, at + 1); i.dispatchEvent(new Event('select', {{ bubbles: true }})); i.dispatchEvent(new KeyboardEvent('keyup', {{ key: 'ArrowRight', bubbles: true }})); return true; }})()",
            js(&input(name)),
            js(word)
        ))
        .await;
    assert!(ok, "{name} has {word}");
}

async fn present(c: &Ctx, css: &str) -> bool {
    c.ui.count(&format!("{EDITOR} {css}")).await > 0
}

async fn no_csp_violation(c: &mut Ctx) {
    start(c).await;
    replace_in(c, MAIN, "max_connections: 12", "max_connections: 13").await;
    dirty(c, MAIN, true).await;
    caret_on(c, MAIN, "max_connections").await;
    key(c, MAIN, "k", "KeyK", "ctrlKey: true,").await;
    key(c, MAIN, "i", "KeyI", "ctrlKey: true,").await;
    let hover = present(c, ".config-editor__hover").await;
    key(c, MAIN, "Escape", "Escape", "").await;
    caret_on(c, MAIN, "max_connections").await;
    key(c, MAIN, " ", "Space", "ctrlKey: true,").await;
    let completions = present(c, ".config-editor__completions").await;
    key(c, MAIN, "Escape", "Escape", "").await;
    key(c, MAIN, "f", "KeyF", "ctrlKey: true,").await;
    let find = present(
        c,
        ".config-editor__search-highlights, [role=search], .config-editor__find",
    )
    .await;
    key(c, MAIN, "Escape", "Escape", "").await;
    if present(c, ".config-editor__diff-change-trigger").await {
        c.ui.click(&format!("{EDITOR} .config-editor__diff-change-trigger"))
            .await;
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let peek = present(c, ".config-editor__diff-peek").await;
    if present(c, ".config-editor__fold-toggle").await {
        c.ui.click(&format!("{EDITOR} .config-editor__fold-toggle"))
            .await;
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let folded = present(c, ".config-editor__fold-chip").await;
    c.ui.shot("text-edit-tools").await;
    let found: Vec<String> = c.ui.eval("window.__csp || []").await;
    assert!(
        found.is_empty(),
        "the editor breaks burrow's Content Security Policy (hover {hover}, completions {completions}, find {find}, change preview {peek}, fold {folded}): {found:#?}"
    );
    let errors = c.ui.take_errors();
    let csp = errors
        .iter()
        .filter(|e| e.contains("Content Security Policy") || e.contains("Refused to apply"))
        .count();
    assert!(
        errors.is_empty(),
        "the page logged errors with the editor open, {csp} of them about the Content Security Policy: {errors:#?}"
    );
    c.ui.click("[data-testid=file-cancel]").await;
    c.ui.click("[data-testid=file-discard]").await;
    c.ui.wait_for(&format!("{CARD}[data-editing=false]"), SECS)
        .await;
}

fn placeholder_after(text: &str, key: &str) -> String {
    let at = text.find(key).expect("the key") + key.len();
    let rest = &text[at..];
    let start = rest.find('«').expect("a placeholder");
    let end = rest[start..].find('»').expect("its end") + start + '»'.len_utf8();
    rest[start..end].to_string()
}

async fn shots(c: &mut Ctx, step: &str) {
    for (w, size) in [(1440, "1440"), (390, "390")] {
        c.ui.viewport(w, 900).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let wide: bool = c
            .ui
            .eval("document.documentElement.scrollWidth > document.documentElement.clientWidth")
            .await;
        assert!(!wide, "{step} at {w}px scrolls sideways");
        let inside: bool = c
            .ui
            .eval(&format!(
                "(() => {{ const e = document.querySelector({}).getBoundingClientRect(); const card = document.querySelector({}).getBoundingClientRect(); return e.right <= card.right + 0.5 && e.left >= card.left - 0.5 && e.width >= card.width - 40; }})()",
                js(EDITOR),
                js(CARD)
            ))
            .await;
        assert!(inside, "{step} at {w}px: the editor fills its card");
        c.ui.shot(&format!("{step}-{size}")).await;
    }
    c.ui.viewport(1440, 900).await;
}

fn secrets_kept(c: &Ctx, tool_before: &str) {
    let text = c.world.config();
    for (name, s) in &c.world.secrets {
        assert!(text.contains(s.as_str()), "the secret of {name} is kept");
    }
    assert!(text.contains("# keep this comment"), "comments survive");
    let tool = std::fs::read_to_string(c.world.file(TOOL)).expect("tool.yaml");
    assert_eq!(tool, tool_before, "the read-only drop-in is untouched");
}

async fn opens_with_one_tab_per_file(c: &mut Ctx) {
    start(c).await;
    assert_eq!(
        c.ui.all_attrs(&format!("{EDITOR} [role=tab]"), "data-document-id")
            .await,
        [MAIN, APPS, TOOL]
    );
    assert!(!c.ui.enabled(REVIEW).await, "nothing to review yet");
    let main = text_of(c, MAIN).await;
    assert!(main.contains("«secret ") && main.contains("# keep this comment"));
    for s in c.world.secrets.values() {
        assert!(!main.contains(s.as_str()), "a secret shows in the editor");
    }
    shots(c, "text-edit-open").await;
    show(c, TOOL).await;
    c.ui.wait_text("[data-testid=file-read-only]", TOOL, SECS)
        .await;
    assert_eq!(
        c.ui.attr_of(&input(TOOL), "aria-readonly").await,
        "true",
        "the read-only drop-in says so"
    );
    let tool = text_of(c, TOOL).await;
    let _: bool = c
        .ui
        .eval(&format!(
            "(() => {{ const i = document.querySelector({}); i.focus(); i.setSelectionRange(0, 0); i.dispatchEvent(new InputEvent('beforeinput', {{ bubbles: true, cancelable: true, inputType: 'insertText', data: 'x' }})); return true; }})()",
            js(&input(TOOL))
        ))
        .await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        text_of(c, TOOL).await,
        tool,
        "the read-only drop-in cannot be typed in"
    );
    dirty(c, TOOL, false).await;
    assert!(
        !c.ui.enabled(REVIEW).await,
        "nothing to review after typing in it"
    );
    shots(c, "text-edit-read-only-file").await;
    show(c, MAIN).await;
    assert_eq!(c.ui.count("[data-testid=file-read-only]").await, 0);
}

async fn edit_review_apply(c: &mut Ctx, tool_before: &str) {
    replace_in(c, MAIN, "max_connections: 100", "max_connections: 120").await;
    dirty(c, MAIN, true).await;
    c.ui.wait_for(&format!("{REVIEW}:not([disabled])"), SECS)
        .await;
    shots(c, "text-edit-dirty").await;
    save_key(c, MAIN).await;
    c.ui.wait_for("[data-testid=review]", CHECKED).await;
    let review = c.ui.text("[data-testid=review]").await;
    assert!(review.contains("max_connections"), "{review}");
    for s in c.world.secrets.values() {
        assert!(!review.contains(s.as_str()), "a secret shows on Review");
    }
    c.ui.shot("text-edit-review").await;
    apply_now(c, "Applied: now the version of").await;
    c.ui.wait_for(&format!("{CARD}[data-editing=false]"), SECS)
        .await;
    c.ui.wait_text("[data-testid=file-text] pre", "max_connections: 120", SECS)
        .await;
    assert!(c.world.config().contains("max_connections: 120"));
    secrets_kept(c, tool_before);
}

async fn a_moved_placeholder_is_refused(c: &mut Ctx, tool_before: &str) {
    let before = c.world.config();
    start(c).await;
    let main = text_of(c, MAIN).await;
    let home = placeholder_after(&main, "home:\n    secret: ");
    let office = placeholder_after(&main, "office:\n    secret: ");
    assert_ne!(home, office);
    replace_in(
        c,
        MAIN,
        &format!("secret: {office}"),
        &format!("secret: {home}"),
    )
    .await;
    dirty(c, MAIN, true).await;
    let now = text_of(c, MAIN).await;
    let at = now.find("office:").unwrap_or(0);
    println!(
        "moved {home} over {office}; the editor's text there: {:?}",
        &now[at..(at + 80).min(now.len())]
    );
    burrow_problem(c, MAIN, "placeholder").await;
    shots(c, "text-edit-moved-placeholder").await;
    c.ui.click(REVIEW).await;
    c.ui.wait_true(
        "!!document.querySelector('[data-testid=review] [data-testid=apply][disabled]') || !!document.querySelector('[data-testid=config-file][data-editing=true]')",
        "the moved placeholder cannot be applied",
        CHECKED,
    )
    .await;
    if c.ui.count("[data-testid=review]").await > 0 {
        c.ui.click("[data-testid=keep-editing]").await;
        c.ui.wait_for(&format!("{CARD}[data-editing=true]"), SECS)
            .await;
    }
    assert_eq!(c.world.config(), before, "nothing was written");
    secrets_kept(c, tool_before);
}

async fn cancel_asks_when_edited(c: &mut Ctx) {
    let before = c.world.config();
    c.ui.click("[data-testid=file-cancel]").await;
    c.ui.wait_for("[data-testid=file-cancel-dialog]", SECS)
        .await;
    c.ui.shot("text-edit-cancel-asks").await;
    c.ui.click("[data-testid=file-keep]").await;
    c.ui.wait_gone("[data-testid=file-cancel-dialog]", SECS)
        .await;
    assert_eq!(c.ui.count(&format!("{CARD}[data-editing=true]")).await, 1);
    c.ui.click("[data-testid=file-cancel]").await;
    c.ui.click("[data-testid=file-discard]").await;
    c.ui.wait_for(&format!("{CARD}[data-editing=false]"), SECS)
        .await;
    assert_eq!(c.world.config(), before);
    start(c).await;
    dirty(c, MAIN, false).await;
    c.ui.click("[data-testid=file-cancel]").await;
    c.ui.wait_for(&format!("{CARD}[data-editing=false]"), SECS)
        .await;
    assert_eq!(
        c.ui.count("[data-testid=file-cancel-dialog]").await,
        0,
        "a clean edit closes without asking"
    );
}

async fn a_change_on_the_server_shows_the_notice(c: &mut Ctx) {
    start(c).await;
    replace_in(c, APPS, "target: 22", "target: 2222").await;
    show(c, APPS).await;
    dirty(c, APPS, true).await;
    std::fs::write(
        c.world.file(APPS),
        std::fs::read_to_string(c.world.file(APPS))
            .expect("apps.yaml")
            .replace("target: 22", "target: 23"),
    )
    .expect("a hand edit of the drop-in");
    let notice =
        format!("{EDITOR} [role=tabpanel][data-document-id='{APPS}'] .config-editor__notice");
    c.ui.wait_for(&notice, Duration::from_secs(20)).await;
    shots(c, "text-edit-changed-on-server").await;
    c.ui.click(&format!(
        "{notice} .config-editor__notice-actions button:last-child"
    ))
    .await;
    c.ui.click("[data-testid=file-cancel]").await;
    c.ui.click("[data-testid=file-discard]").await;
    c.ui.wait_for(&format!("{CARD}[data-editing=false]"), SECS)
        .await;
}

async fn a_locked_field_is_refused(c: &mut Ctx) {
    let before = c.world.config();
    start(c).await;
    let main = text_of(c, MAIN).await;
    let last = main.lines().last().expect("a last line").to_string();
    replace_in(c, MAIN, &last, &format!("{last}\nupdate:\n  remote: true")).await;
    dirty(c, MAIN, true).await;
    burrow_problem(c, MAIN, "lock").await;
    shots(c, "text-edit-locked").await;
    c.ui.click("[data-testid=file-cancel]").await;
    c.ui.click("[data-testid=file-discard]").await;
    assert_eq!(c.world.config(), before);
}

async fn waits_for_changes_in_review(c: &mut Ctx) {
    c.ui.nav("/services/web").await;
    super::edit_mode(c).await;
    c.ui.fill("#f-services-web-target", "4000").await;
    super::wait_checked(c).await;
    super::review_now(c).await;
    c.ui.wait_for("[data-testid=review]", CHECKED).await;
    c.ui.click("[data-testid=keep-editing]").await;
    c.ui.nav("/settings?tab=file").await;
    c.ui.click("[data-testid=file-edit]").await;
    c.ui.wait_text("[data-testid=file-waiting]", "waiting in Review", SECS)
        .await;
    assert_eq!(c.ui.count(&format!("{CARD}[data-editing=true]")).await, 0);
    shots_plain(c, "text-edit-waiting").await;
    c.ui.click("[data-testid=file-waiting-link]").await;
    c.ui.wait_for("[data-testid=review]", CHECKED).await;
    c.ui.click("[data-testid=discard]").await;
}

async fn shots_plain(c: &mut Ctx, step: &str) {
    c.ui.shot(step).await;
    c.ui.viewport(390, 900).await;
    c.ui.shot(&format!("{step}-390")).await;
    c.ui.viewport(1440, 900).await;
}

async fn placeholders_follow_a_restart(c: &mut Ctx, tool_before: &str) {
    start(c).await;
    let before = placeholder_after(&text_of(c, MAIN).await, "Authorization: ");
    assert!(
        !text_of(c, MAIN).await.contains(HEADER),
        "the header value shows"
    );
    c.world.stop("server");
    c.ui.wait_text(PILL, "Reconnecting", Duration::from_secs(15))
        .await;
    c.world
        .start("server", &["server", "-c", "server.yaml"], &[]);
    c.ui.wait_text(PILL, "Connected", Duration::from_secs(30))
        .await;
    c.ui.wait_true(
        &format!(
            "(() => {{ const t = document.querySelector({}).value; return t.includes('Authorization: ') && !t.includes({}); }})()",
            js(&input(MAIN)),
            js(&before)
        ),
        "the editor takes the restarted server's placeholders",
        CHECKED,
    )
    .await;
    replace_in(c, MAIN, "max_connections: 120", "max_connections: 125").await;
    dirty(c, MAIN, true).await;
    c.ui.wait_for(&format!("{REVIEW}:not([disabled])"), SECS)
        .await;
    c.ui.click(REVIEW).await;
    c.ui.wait_for("[data-testid=review]", CHECKED).await;
    apply_now(c, "Applied: now the version of").await;
    let text = c.world.config();
    assert!(text.contains("max_connections: 125"), "{text}");
    assert!(text.contains(HEADER), "the header keeps its value: {text}");
    secrets_kept(c, tool_before);
}

fn text_edit(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        let _: bool = c.ui.eval(WATCH_CSP).await;
        let tool_before = std::fs::read_to_string(c.world.file(TOOL)).expect("tool.yaml");
        opens_with_one_tab_per_file(c).await;
        edit_review_apply(c, &tool_before).await;
        a_moved_placeholder_is_refused(c, &tool_before).await;
        cancel_asks_when_edited(c).await;
        placeholders_follow_a_restart(c, &tool_before).await;
        a_change_on_the_server_shows_the_notice(c).await;
        a_locked_field_is_refused(c).await;
        waits_for_changes_in_review(c).await;
        no_csp_violation(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_write_admin_edits_the_config_files_as_text_and_applies_them_through_review() {
    run("text-edit", opts(), false, text_edit);
}
