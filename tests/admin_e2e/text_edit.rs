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
    let styled: (usize, String) = c
        .ui
        .eval(&format!(
            "(() => {{ const e = document.querySelector('{EDITOR} .config-editor, {EDITOR} .config-editor-documents'); return [document.querySelectorAll('{EDITOR} style').length, e ? getComputedStyle(e).getPropertyValue('--config-editor-bg').trim() : '']; }})()"
        ))
        .await;
    assert!(
        styled.0 > 0 && !styled.1.is_empty(),
        "the editor injects its own stylesheet and it applies: {styled:?}"
    );
    replace_in(c, MAIN, "max_connections: 1", "max_connections: 9").await;
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

const IN_PLACE: &str = "(() => { const h = document.querySelector('[data-testid=header]').getBoundingClientRect(); const inner = [document.body, document.querySelector('main'), document.querySelector('[data-testid=config-file]')].filter(e => e && e.scrollTop !== 0).map(e => e.tagName + ' ' + e.scrollTop); return { top: h.top, scrolled: scrollY, inner, max: document.documentElement.scrollHeight - innerHeight }; })()";

async fn page_in_place(c: &Ctx, step: &str) {
    let got: serde_json::Value = c.ui.eval(IN_PLACE).await;
    println!("{step}: {got}");
    assert_eq!(
        got["top"].as_f64(),
        Some(0.0),
        "{step}: the header leaves the top: {got}"
    );
    assert_eq!(
        got["inner"],
        serde_json::json!([]),
        "{step}: only the document scrolls: {got}"
    );
    assert!(
        got["scrolled"].as_f64() <= got["max"].as_f64().map(|m| m.max(0.0)),
        "{step}: the document scrolls past its end: {got}"
    );
}

async fn typing_keeps_the_page_in_place(c: &mut Ctx) {
    for w in [1440, 390] {
        c.ui.viewport(w, 900).await;
        let _: bool =
            c.ui.eval("(() => { scrollTo(0, 0); return true; })()")
                .await;
        let last = text_of(c, MAIN)
            .await
            .lines()
            .last()
            .unwrap_or_default()
            .to_string();
        replace_in(c, MAIN, &last, &format!("{last} ")).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        page_in_place(c, &format!("typing at the last line at {w}px")).await;
        replace_in(c, MAIN, &format!("{last} "), &last).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    c.ui.viewport(1440, 900).await;
}

fn placeholder_after(text: &str, key: &str) -> String {
    let at = text.find(key).expect("the key") + key.len();
    let rest = &text[at..];
    let start = rest.find('«').expect("a placeholder");
    let end = rest[start..].find('»').expect("its end") + start + '»'.len_utf8();
    rest[start..end].to_string()
}

async fn shots(c: &mut Ctx, step: &str) {
    let mut first_space: Option<f64> = None;
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
        let band: serde_json::Value = c
            .ui
            .eval(&format!(
                "(() => {{ const e = document.querySelector({}).getBoundingClientRect(); const h = document.querySelector('[data-testid=file-head]').getBoundingClientRect(); const space = parseFloat(getComputedStyle(document.querySelector('[data-testid=file-body]')).paddingTop); const n = document.querySelector('[data-testid=file-notes]'); const r = n && n.getBoundingClientRect(); return {{ space, above: (r ? r.top : e.top) - h.bottom, below: r ? e.top - r.bottom : null }}; }})()",
                js(EDITOR)
            ))
            .await;
        let space = band["space"].as_f64().unwrap_or(0.0);
        assert!(
            space > 0.0,
            "{step} at {w}px: a small space under the card head: {band}"
        );
        assert_eq!(
            *first_space.get_or_insert(space),
            space,
            "{step} at {w}px: the same space at every width"
        );
        let near = |v: &serde_json::Value| v.as_f64().is_some_and(|x| (x - space).abs() < 0.5);
        assert!(
            near(&band["above"]) && (band["below"].is_null() || near(&band["below"])),
            "{step} at {w}px: the notes sit in the band between the head and the editor: {band}"
        );
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
    replace_in(c, MAIN, &office, "").await;
    replace_in(
        c,
        MAIN,
        "office:\n    secret: ",
        &format!("office:\n    secret: {home}"),
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
    page_in_place(c, "the moved placeholder").await;
    typing_keeps_the_page_in_place(c).await;
    c.ui.view_shot("text-edit-moved-placeholder-viewport").await;
    shots(c, "text-edit-moved-placeholder").await;
    c.ui.click(REVIEW).await;
    let refused = "!!document.querySelector('[data-testid=review] [data-testid=apply][disabled]') || !!document.querySelector('[data-testid=config-file][data-editing=true]')";
    let started = std::time::Instant::now();
    while !c.ui.eval::<bool>(refused).await {
        if started.elapsed() > CHECKED {
            let page: String = c
                .ui
                .eval("location.pathname + location.search + ' | ' + [...document.querySelectorAll('[data-testid]')].map(e => e.dataset.testid).filter(t => /review|file|check|apply|toast/.test(t)).join(',') + ' | ' + (document.querySelector('main')?.innerText ?? '').slice(0, 600)")
                .await;
            panic!("the moved placeholder cannot be applied, but the page is: {page}");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
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
    show(c, APPS).await;
    replace_in(c, APPS, "target: 22", "target: 2222").await;
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

async fn click_toast(c: &Ctx, label: &str) {
    let ok: bool = c
        .ui
        .eval(&format!(
            "(() => {{ const b = [...document.querySelectorAll('[data-testid=toasts] button')].find(b => b.textContent.trim() === {}); if (!b) return false; b.click(); return true; }})()",
            js(label)
        ))
        .await;
    assert!(ok, "a toast offers {label}");
}

async fn text_and_form_edits_wait_for_each_other(c: &mut Ctx, tool_before: &str) {
    start(c).await;
    replace_in(
        c,
        MAIN,
        "# keep this comment",
        "# keep this comment, edited as text",
    )
    .await;
    dirty(c, MAIN, true).await;
    c.ui.nav("/services/web").await;
    super::edit_mode(c).await;
    c.ui.fill("#f-services-web-target", "4000").await;
    c.ui.click("[data-testid=save-bar] [data-testid=save]")
        .await;
    c.ui.wait_text("[data-testid=toasts]", "text edits first", SECS)
        .await;
    assert_eq!(
        c.ui.count("[data-testid=save-bar]").await,
        1,
        "Add to changes does not add while the text waits"
    );
    shots_plain(c, "text-edit-form-waits").await;
    click_toast(c, "Open Config file").await;
    c.ui.wait_for(&format!("{CARD}[data-editing=true]"), SECS)
        .await;
    dirty(c, MAIN, true).await;
    c.ui.click(REVIEW).await;
    c.ui.wait_for("[data-testid=review]", CHECKED).await;
    apply_now(c, "Applied: now the version of").await;
    let text = c.world.config();
    assert!(text.contains("edited as text"), "{text}");
    secrets_kept(c, tool_before);
    c.ui.nav("/services/web").await;
    c.ui.click("[role=tablist] [data-tab=settings]").await;
    c.ui.wait_for("[data-testid=save-bar]", SECS).await;
    c.ui.wait_for("#f-services-web-target", SECS).await;
    let typed: String =
        c.ui.eval("document.querySelector('#f-services-web-target').value")
            .await;
    assert_eq!(
        typed, "4000",
        "the form edit not added yet survives the text's Apply"
    );
    c.ui.click("[data-testid=save-bar] [data-testid=save]")
        .await;
    c.ui.wait_text("[data-testid=toasts]", "Added to changes", SECS)
        .await;
    super::review_now(c).await;
    c.ui.wait_for("[data-testid=review]", CHECKED).await;
    c.ui.click("[data-testid=discard]").await;
}

async fn placeholders_follow_a_restart(c: &mut Ctx, tool_before: &str) {
    start(c).await;
    let before = placeholder_after(&text_of(c, MAIN).await, "Authorization: ");
    assert!(
        !text_of(c, MAIN).await.contains(HEADER),
        "the header value shows"
    );
    replace_in(c, MAIN, "max_connections: 120", "max_connections: 125").await;
    dirty(c, MAIN, true).await;
    c.world.stop("server");
    c.ui.wait_text(PILL, "Reconnecting", Duration::from_secs(15))
        .await;
    c.world
        .start("server", &["server", "-c", "server.yaml"], &[]);
    c.ui.wait_text(PILL, "Connected", Duration::from_secs(30))
        .await;
    c.ui.wait_true(
        &format!(
            "(() => {{ const t = document.querySelector({}).value; return /Authorization: \"?«secret [0-9a-f]{{8}}»/.test(t) && !t.includes({}) && t.includes('max_connections: 125'); }})()",
            js(&input(MAIN)),
            js(&before)
        ),
        "the typed text takes the restarted server's placeholders",
        CHECKED,
    )
    .await;
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

const EDIT_AS: &str = "[data-testid=file-edit-as]";

async fn js_click(c: &Ctx, css: &str) {
    let ok: bool = c
        .ui
        .eval(&format!(
            "(() => {{ const e = document.querySelector({}); if (!e) return false; e.click(); return true; }})()",
            js(css)
        ))
        .await;
    assert!(ok, "{css} is there to click");
}

async fn edit_as(c: &Ctx, label: &str, shown: &str) {
    c.ui.select("#file-edit-as", label).await;
    c.ui.wait_for(&format!("{EDIT_AS}[data-shown={shown}]"), SECS)
        .await;
}

async fn scroll_y(c: &Ctx) -> f64 {
    c.ui.eval("scrollY").await
}

async fn kept_scroll(c: &Ctx, was: f64, step: &str) -> f64 {
    let (now, max): (f64, f64) =
        c.ui.eval("[scrollY, Math.max(0, document.documentElement.scrollHeight - innerHeight)]")
            .await;
    assert_eq!(
        now,
        was.min(max),
        "{step}: the editor focusing itself leaves the page where it is (only a shorter page moves it)"
    );
    now
}

async fn focus_leaves_the_page_where_it_is(c: &mut Ctx) {
    let _: bool =
        c.ui.eval("(() => { scrollTo(0, 120); return true; })()")
            .await;
    let at = scroll_y(c).await;
    assert_eq!(at, 120.0, "the page is long enough to scroll");
    js_click(c, &tab(APPS)).await;
    c.ui.wait_for(&format!("{}[aria-selected=true]", tab(APPS)), SECS)
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let at = kept_scroll(c, at, "a switch to a short file").await;
    js_click(c, &tab(MAIN)).await;
    c.ui.wait_for(&format!("{}[aria-selected=true]", tab(MAIN)), SECS)
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    kept_scroll(c, at, "a switch back").await;
}

fn number_after(text: &str, key: &str) -> String {
    let at = text.find(key).expect("the key") + key.len();
    text[at..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect()
}

async fn edit_as_json_and_toml(c: &mut Ctx, tool_before: &str) {
    start(c).await;
    focus_leaves_the_page_where_it_is(c).await;
    assert_eq!(
        c.ui.all_attrs("#file-edit-as option", "value").await,
        ["YAML", "JSON", "TOML"],
        "the file's own language comes first"
    );
    edit_as(c, "JSON", "JSON").await;
    let json = text_of(c, MAIN).await;
    assert!(json.trim_start().starts_with('{'), "{json}");
    let max = number_after(&json, "\"max_connections\": ");
    replace_in(
        c,
        MAIN,
        &format!("\"max_connections\": {max}"),
        "\"max_connections\": 140",
    )
    .await;
    dirty(c, MAIN, true).await;
    edit_as(c, "TOML", "TOML").await;
    let toml = text_of(c, MAIN).await;
    assert!(toml.contains("max_connections = 140"), "{toml}");
    assert!(toml.contains("«secret "), "placeholders stay in TOML");
    let home = placeholder_after(&toml, "[clients.home]\nsecret = \"");
    let office = placeholder_after(&toml, "[clients.office]\nsecret = \"");
    replace_in(c, MAIN, &office, "").await;
    replace_in(
        c,
        MAIN,
        "[clients.office]\nsecret = \"",
        &format!("[clients.office]\nsecret = \"{home}"),
    )
    .await;
    burrow_problem(c, MAIN, "placeholder").await;
    shots(c, "text-edit-as-toml").await;
    replace_in(
        c,
        MAIN,
        &format!("[clients.office]\nsecret = \"{home}"),
        "[clients.office]\nsecret = \"",
    )
    .await;
    replace_in(
        c,
        MAIN,
        "[clients.office]\nsecret = \"",
        &format!("[clients.office]\nsecret = \"{office}"),
    )
    .await;
    c.ui.wait_for(&format!("{REVIEW}:not([disabled])"), CHECKED)
        .await;
    c.ui.click(REVIEW).await;
    c.ui.wait_for("[data-testid=review]", CHECKED).await;
    let review = c.ui.text("[data-testid=review]").await;
    assert!(
        review.contains("max_connections: 140"),
        "Review shows YAML: {review}"
    );
    apply_now(c, "Applied: now the version of").await;
    let text = c.world.config();
    assert!(text.contains("max_connections: 140"), "{text}");
    assert!(
        text.starts_with("# server.yaml"),
        "still YAML with its comments: {text}"
    );
    secrets_kept(c, tool_before);
    start(c).await;
    let last = text_of(c, MAIN)
        .await
        .lines()
        .last()
        .unwrap_or_default()
        .to_string();
    replace_in(c, MAIN, &last, &format!("{last}\nheartbeat:")).await;
    dirty(c, MAIN, true).await;
    c.ui.select("#file-edit-as", "TOML").await;
    let reason = "(() => [...document.querySelectorAll('[data-testid=file-editor] .config-editor__diagnostic-marker')].some(m => /null/i.test(m.dataset.message || '')))()";
    c.ui.wait_true(reason, "the editor says why TOML waits", SECS)
        .await;
    assert_eq!(c.ui.attr_of(EDIT_AS, "data-shown").await, "YAML");
    c.ui.wait_text("[data-testid=file-edit-as-waits]", "TOML waits", SECS)
        .await;
    shots(c, "text-edit-as-waits").await;
    c.ui.click("[data-testid=file-cancel]").await;
    c.ui.click("[data-testid=file-discard]").await;
    c.ui.wait_for(&format!("{CARD}[data-editing=false]"), SECS)
        .await;
}

async fn a_drop_in_takes_only_items(c: &mut Ctx) {
    start(c).await;
    show(c, APPS).await;
    replace_in(c, APPS, "services:", "listen: 127.0.0.1:1\nservices:").await;
    dirty(c, APPS, true).await;
    let flagged = format!(
        "(() => [...document.querySelectorAll({})].some(m => /listen/.test(m.dataset.message || '')))()",
        js(&format!(
            "{EDITOR} [role=tabpanel][data-document-id='{APPS}'] .config-editor__diagnostic-marker"
        ))
    );
    c.ui.wait_true(
        &flagged,
        "a server setting in a drop-in is flagged",
        CHECKED,
    )
    .await;
    c.ui.click("[data-testid=file-cancel]").await;
    c.ui.click("[data-testid=file-discard]").await;
    c.ui.wait_for(&format!("{CARD}[data-editing=false]"), SECS)
        .await;
}

const ADD: &str = "[data-testid=file-editor] .config-editor-documents__add";

fn close_of(name: &str) -> String {
    format!("{EDITOR} [role=tab][data-document-id='{name}'] .config-editor-documents__close")
}

async fn name_new_file(c: &Ctx, name: &str) {
    c.ui.click(ADD).await;
    c.ui.wait_for("[data-testid=file-new-dialog]", SECS).await;
    c.ui.fill("#file-new-name", name).await;
}

async fn a_new_drop_in_file(c: &mut Ctx, tool_before: &str) {
    start(c).await;
    assert_eq!(
        c.ui.count(&format!("{EDITOR} .config-editor-documents__close"))
            .await,
        0,
        "the files on the server have no close button"
    );
    c.ui.viewport(390, 900).await;
    let reachable: bool = c
        .ui
        .eval(&format!(
            "(() => {{ const r = document.querySelector({}).getBoundingClientRect(); return r.width > 0 && r.right <= innerWidth && r.left >= 0; }})()",
            js(ADD)
        ))
        .await;
    assert!(reachable, "the + is on screen at 390px");
    c.ui.viewport(1440, 900).await;
    name_new_file(c, "my apps").await;
    c.ui.wait_text("[data-testid=file-new-problem]", "not a file name", SECS)
        .await;
    assert!(!c.ui.enabled("[data-testid=file-new-add]").await);
    c.ui.fill("#file-new-name", "apps").await;
    c.ui.wait_text("[data-testid=file-new-problem]", "already exists", SECS)
        .await;
    c.ui.shot("text-edit-new-file-name").await;
    c.ui.fill("#file-new-name", "extra").await;
    c.ui.wait_gone("[data-testid=file-new-problem]", SECS).await;
    c.ui.click("[data-testid=file-new-add]").await;
    let new = "server.d/extra.yaml";
    c.ui.wait_for(&input(new), SECS).await;
    c.ui.wait_for(&format!("{}[aria-selected=true]", tab(new)), SECS)
        .await;
    c.ui.wait_text(
        "[data-testid=file-not-created]",
        "is empty, so it is not created",
        SECS,
    )
    .await;
    c.ui.click(&close_of(new)).await;
    c.ui.wait_gone(&tab(new), SECS).await;
    assert_eq!(
        c.ui.count("[data-testid=file-close-dialog]").await,
        0,
        "an empty new file closes without asking"
    );
    name_new_file(c, "extra").await;
    c.ui.wait_gone("[data-testid=file-new-problem]", SECS).await;
    c.ui.click("[data-testid=file-new-add]").await;
    c.ui.wait_for(&input(new), SECS).await;
    replace_in(
        c,
        new,
        "",
        "services:\n  extra:\n    client: home\n    target: 2223\n    visitors: [laptop]\n",
    )
    .await;
    dirty(c, new, true).await;
    c.ui.click(&close_of(new)).await;
    c.ui.wait_for("[data-testid=file-close-dialog]", SECS).await;
    c.ui.click("[data-testid=file-close-keep]").await;
    c.ui.wait_for(&tab(new), SECS).await;
    shots(c, "text-edit-new-file").await;
    c.ui.click(REVIEW).await;
    c.ui.wait_for("[data-testid=review]", CHECKED).await;
    c.ui.wait_text(
        "[data-testid=review]",
        "server.d/extra.yaml: new file",
        SECS,
    )
    .await;
    apply_now(c, "Applied: now the version of").await;
    let made = std::fs::read_to_string(c.world.file(new)).expect("the new drop-in");
    assert!(
        made.contains("extra:") && made.contains("target: 2223"),
        "{made}"
    );
    secrets_kept(c, tool_before);
    start(c).await;
    c.ui.wait_for(&tab(new), SECS).await;
    assert_eq!(
        c.ui.count(&close_of(new)).await,
        0,
        "after Apply the new file is a file on the server"
    );
    c.ui.click("[data-testid=file-cancel]").await;
    c.ui.wait_for(&format!("{CARD}[data-editing=false]"), SECS)
        .await;
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
        text_and_form_edits_wait_for_each_other(c, &tool_before).await;
        a_change_on_the_server_shows_the_notice(c).await;
        a_locked_field_is_refused(c).await;
        waits_for_changes_in_review(c).await;
        edit_as_json_and_toml(c, &tool_before).await;
        a_drop_in_takes_only_items(c).await;
        a_new_drop_in_file(c, &tool_before).await;
        no_csp_violation(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_write_admin_edits_the_config_files_as_text_and_applies_them_through_review() {
    run("text-edit", opts(), false, text_edit);
}
