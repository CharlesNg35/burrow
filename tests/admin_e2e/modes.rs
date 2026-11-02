use std::time::Duration;

use super::edits::{Flow, click_js};
use super::world::Options;
use super::{Ctx, adornments_clear, edit_mode, open, run};

const PAGES: &[(&str, &str)] = &[
    ("/services/web", "service"),
    ("/services/nas", "website"),
    ("/clients/home", "client"),
    ("/visitors/ops", "visitor"),
    ("/settings", "settings"),
];

const GROUPS: &str = "(() => { const seen = new Map(); for (const i of document.querySelectorAll('[data-testid=detail-settings] input[type=radio], [data-testid=settings] input[type=radio]')) { const r = i.getBoundingClientRect(); if (!r.width && !r.height && !i.closest('label')) continue; const row = i.closest('[data-schema]'); if (!row) continue; const key = i.name; if (!seen.has(key)) seen.set(key, [row.getAttribute('data-schema'), [], '']); const g = seen.get(key); g[1].push(i.value); if (i.checked) g[2] = i.value; } return [...seen.entries()].map(([n, g]) => [n, g[0], g[1], g[2]]); })()";

fn slug(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

async fn row_shots(c: &mut Ctx, row: &str, step: &str) {
    for (w, h, size) in [(1440, 900, "desktop"), (390, 844, "390")] {
        c.ui.viewport(w, h).await;
        adornments_clear(c, step).await;
        for dark in [false, true] {
            c.ui.dark(dark).await;
            let png = c.ui.png_of(row).await;
            let scheme = if dark { "dark" } else { "light" };
            c.ui.flow.save(&format!("{step}-{size}-{scheme}"), &png);
        }
    }
    c.ui.dark(false).await;
    c.ui.viewport(1440, 900).await;
}

fn modes(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        let mut walked = 0;
        for (path, page) in PAGES {
            c.ui.nav(path).await;
            c.ui.wait_gone("[aria-busy=true]", super::SECS).await;
            if c.ui.count("[data-testid=edit]").await > 0 {
                edit_mode(c).await;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
            let groups: Vec<(String, String, Vec<String>, String)> = c.ui.eval(GROUPS).await;
            for (name, schema, values, checked) in groups {
                let row = format!("[data-schema='{schema}']");
                for v in &values {
                    click_js(c, &format!("input[name='{name}'][value='{v}']")).await;
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    row_shots(
                        c,
                        &row,
                        &format!("mode-{page}-{}-{}", slug(&schema), slug(v)),
                    )
                    .await;
                    walked += 1;
                }
                if !checked.is_empty() {
                    click_js(c, &format!("input[name='{name}'][value='{checked}']")).await;
                }
            }
        }
        assert!(walked >= 10, "only {walked} field modes were found");
        c.ui.assert_no_errors();
    })
}

#[test]
fn every_mode_of_every_field_editor_keeps_its_text_clear_at_desktop_and_phone_width() {
    let opts = Options {
        extra_top: "web:\n  https: 127.0.0.1:HTTPS_PORT\n".into(),
        extra_services: "  nas:\n    client: home\n    target: 192.168.1.20:443\n    web:\n      hosts: [nas.example.com]\n      tls: passthrough\n".into(),
        ..Options::default()
    };
    run("field-modes", opts, false, modes);
}
