use super::edits::Flow;
use super::world::Options;
use super::{Ctx, SECS, edit_mode, open, run};

const BOX: &str = "[data-schema='services/*/visitors'] [role=combobox]";
const MANY: usize = 1000;

fn opts() -> Options {
    Options {
        extra_visitors: (1..=MANY)
            .map(|i| format!("  guest{i:04}:\n    secret: {}\n", super::world::secret()))
            .collect(),
        ..Options::default()
    }
}

async fn timed(c: &Ctx, act: &str, done: &str) -> f64 {
    let ms: f64 = c
        .ui
        .eval(&format!(
            "(async () => {{ const t0 = performance.now(); {act}; for (let i = 0; i < 20000 && !({done}); i++) await new Promise(r => setTimeout(r, 0)); if (!({done})) return -1; return performance.now() - t0; }})()"
        ))
        .await;
    assert!(ms >= 0.0, "timed out waiting for {done}");
    ms
}

fn rows(list: &str, n: usize, more: bool) -> String {
    format!(
        "(() => {{ const l = document.querySelector('{list}'); return !!l && l.querySelectorAll('[role=option]').length === {n} && !!l.querySelector('[data-more]') === {more}; }})()"
    )
}

fn typed(text: &str) -> String {
    format!(
        "(() => {{ const e = document.querySelector(\"{BOX}\"); e.value = '{text}'; e.dispatchEvent(new Event('input', {{ bubbles: true }})); }})()"
    )
}

fn visitor_scale(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.viewport(1440, 900).await;
        c.ui.nav("/services/ssh").await;
        edit_mode(c).await;
        c.ui.wait_for(BOX, SECS).await;
        let id = c.ui.attr_of(BOX, "id").await;
        let list = format!("#{id}-list");
        let open_ms = timed(
            c,
            &format!("document.querySelector(\"{BOX}\").focus()"),
            &rows(&list, 100, true),
        )
        .await;
        let more = c.ui.text(&format!("{list} [data-more]")).await;
        assert!(more.contains("more, type to narrow"), "{more}");
        let type_ms = timed(c, &typed("guest042"), &rows(&list, 10, false)).await;
        c.ui.view_shot("visitor-scale-1000").await;
        c.ui.press(BOX, "ArrowDown").await;
        c.ui.press(BOX, "Enter").await;
        c.ui.wait_for(
            "[data-schema='services/*/visitors'] [data-chip=guest0421]",
            SECS,
        )
        .await;
        println!(
            "visitor picker with {MANY} visitors: open {open_ms:.1} ms, typing {type_ms:.1} ms"
        );
        assert!(
            open_ms < 500.0 && type_ms < 500.0,
            "open {open_ms} ms, typing {type_ms} ms"
        );
        c.ui.click("[data-testid=discard-edit]").await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_visitor_picker_over_1000_visitors_caps_filters_and_picks() {
    run("visitor-scale", opts(), false, visitor_scale);
}
