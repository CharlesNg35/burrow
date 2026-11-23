use super::edits::Flow;
use super::world::{self, Options};
use super::{Ctx, SECS, edit_mode, open, run};

const CLIENTS: usize = 500;
const VISITORS: usize = 1000;

pub(super) fn opts(n: usize) -> Options {
    let visitors: String = (1..=VISITORS)
        .map(|i| format!("  v{i:04}:\n    secret: {}\n", world::secret()))
        .collect();
    Options {
        extra_services: (1..=n)
            .map(|i| {
                format!(
                    "  s{i:04}:\n    client: c{:03}\n    target: {}\n    visitors: [laptop, v{:04}]\n",
                    i % CLIENTS + 1,
                    10000 + i,
                    i % VISITORS + 1
                )
            })
            .collect(),
        extra_clients: (1..=CLIENTS)
            .map(|i| format!("  c{i:03}:\n    secret: {}\n", world::secret()))
            .collect(),
        drop_ins: vec![("visitors.yaml", format!("visitors:\n{visitors}"), false)],
        ..Options::default()
    }
}

#[derive(Debug, serde::Deserialize)]
struct Numbers {
    first: f64,
    last: f64,
    load_long_max: f64,
    load_long_total: f64,
    idle_long_max: f64,
    idle_long_total: f64,
    idle_frame_max: f64,
    idle_frames_over_20: u32,
    scroll_long_max: f64,
    scroll_long_total: f64,
    scroll_frame_max: f64,
    scroll_frames_over_50: u32,
    rows_in_dom: u32,
    heap_mb: f64,
}

impl Numbers {
    fn line(&self) -> String {
        format!(
            "first {:.0} ms, last {:.0} ms; load long tasks max {:.0} / total {:.0} ms; idle long max {:.0} / total {:.0} ms, frame max {:.0} ms, frames over 20 ms {}; scroll long max {:.0} / total {:.0} ms, frame max {:.0} ms, frames over 50 ms {}; {} rows in the page; heap {:.0} MB",
            self.first,
            self.last,
            self.load_long_max,
            self.load_long_total,
            self.idle_long_max,
            self.idle_long_total,
            self.idle_frame_max,
            self.idle_frames_over_20,
            self.scroll_long_max,
            self.scroll_long_total,
            self.scroll_frame_max,
            self.scroll_frames_over_50,
            self.rows_in_dom,
            self.heap_mb
        )
    }
}

const MEASURE: &str = r#"(() => { window.__scale = null; (async () => {
  const long = [];
  const seen = new PerformanceObserver(l => l.getEntries().forEach(e => long.push(e.duration)));
  seen.observe({ type: 'longtask' });
  const frame = () => new Promise(r => requestAnimationFrame(() => r(performance.now())));
  const until = async (f, ms) => { const end = performance.now() + ms; while (!f()) { if (performance.now() > end) return -1; await frame(); } return performance.now(); };
  const run = (location.pathname.match(/^\/r-[0-9a-f]+/) || [''])[0];
  const t0 = performance.now();
  history.pushState(null, '', run + '/services');
  dispatchEvent(new PopStateEvent('popstate'));
  const list = () => document.querySelector('[data-testid=list]');
  const first = await until(() => list() && list().querySelector('[data-row]:not([hidden])'), 60000) - t0;
  const last = await until(() => { const l = list(); l.scrollTop = l.scrollHeight; const r = l.querySelector('[data-row=LAST]'); return r && !r.hidden && r.getBoundingClientRect().height > 0; }, 120000) - t0;
  await new Promise(r => setTimeout(r, 1500));
  const load = long.splice(0);
  const gaps = [];
  let was = await frame();
  const quiet = performance.now() + 5000;
  while (performance.now() < quiet) {
    const now = await frame();
    gaps.push(now - was);
    was = now;
  }
  const idle = long.splice(0);
  const l = list();
  l.scrollTop = 0;
  await frame();
  const frames = [];
  let prev = await frame();
  for (let i = 0; i < 120; i++) {
    l.scrollTop += i < 60 ? 600 : -600;
    const now = await frame();
    frames.push(now - prev);
    prev = now;
  }
  await new Promise(r => setTimeout(r, 300));
  const scroll = long.splice(0);
  seen.disconnect();
  const sum = a => a.reduce((x, y) => x + y, 0);
  return {
    first, last,
    load_long_max: Math.max(0, ...load), load_long_total: sum(load),
    idle_long_max: Math.max(0, ...idle), idle_long_total: sum(idle),
    idle_frame_max: Math.max(...gaps), idle_frames_over_20: gaps.filter(g => g > 20).length,
    scroll_long_max: Math.max(0, ...scroll), scroll_long_total: sum(scroll),
    scroll_frame_max: Math.max(...frames), scroll_frames_over_50: frames.filter(f => f > 50).length,
    rows_in_dom: l.querySelectorAll('[data-row]').length,
    heap_mb: performance.memory ? performance.memory.usedJSHeapSize / 1048576 : -1,
  };
})().then(r => { window.__scale = r; }); return true; })()"#;

const IDLE: &str = r#"new Promise(r => { const long = []; const gaps = []; const o = new PerformanceObserver(l => l.getEntries().forEach(e => long.push(e.duration))); o.observe({ type: 'longtask' }); let was = performance.now(); const end = was + 5000; const step = now => { gaps.push(now - was); was = now; if (now < end) { requestAnimationFrame(step); } else { o.disconnect(); r(`long max ${Math.max(0, ...long)} ms, frame max ${Math.round(Math.max(...gaps))} ms`); } }; requestAnimationFrame(step); })"#;

async fn idle_on(c: &Ctx, page: &str) -> String {
    c.ui.nav(page).await;
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    c.ui.eval(IDLE).await
}

async fn measure(c: &mut Ctx, n: usize) -> Numbers {
    open(c, "ops").await;
    c.ui.viewport(1440, 900).await;
    let js = MEASURE.replace("LAST", &format!("s{n:04}"));
    let _: bool = c.ui.eval(&js).await;
    c.ui.wait_true(
        "!!window.__scale",
        "the list measurement to finish",
        std::time::Duration::from_secs(300),
    )
    .await;
    let got: Numbers = c.ui.eval("window.__scale").await;
    let mut note = format!("list-scale {n} services: {}", got.line());
    for page in ["/", "/clients", "/visitors", "/about"] {
        note.push_str(&format!(" | {page} idle {}", idle_on(c, page).await));
    }
    note.push('\n');
    let file = super::sheet::shots_dir().join("list-scale.txt");
    let old = std::fs::read_to_string(&file).unwrap_or_default();
    std::fs::write(&file, old + &note).expect("write the list numbers");
    c.ui.view_shot(&format!("list-{n}")).await;
    got
}

fn check(n: usize, got: &Numbers) {
    assert!(
        got.first >= 0.0 && got.last >= 0.0,
        "{n}: the list showed its rows: {got:?}"
    );
    assert!(
        got.scroll_long_max < 100.0 && got.scroll_frames_over_50 <= 3,
        "{n}: scrolling the list stays smooth: {got:?}"
    );
    assert!(
        got.rows_in_dom <= 80,
        "{n}: only the rows in and near view are in the page: {got:?}"
    );
    assert!(
        got.idle_long_max < 200.0 && got.heap_mb < 200.0,
        "{n}: an idle list costs little time and memory: {got:?}"
    );
}

fn at_500(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        let got = measure(c, 500).await;
        check(500, &got);
        c.ui.assert_no_errors();
    })
}

async fn in_view(c: &Ctx, key: &str) -> bool {
    c.ui.eval(&format!(
        "(() => {{ const l = document.querySelector('[data-testid=list]').getBoundingClientRect(); const r = document.querySelector('[data-row={key}]').getBoundingClientRect(); return r.top >= l.top - 1 && r.bottom <= l.bottom + 1; }})()"
    ))
    .await
}

async fn far_rows(c: &mut Ctx) {
    c.ui.nav("/services/s4000").await;
    c.ui.wait_for("[data-row=s4000][data-selected=true]", SECS)
        .await;
    c.ui.wait_true(
        "(() => { const l = document.querySelector('[data-testid=list]').getBoundingClientRect(); const r = document.querySelector('[data-row=s4000]').getBoundingClientRect(); return r.top >= l.top - 1 && r.bottom <= l.bottom + 1; })()",
        "a row far down the list is scrolled into view when opened",
        SECS,
    )
    .await;
    let _: bool = c
        .ui
        .eval("(() => { const l = document.querySelector('[data-testid=list]'); l.focus(); l.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true, cancelable: true })); return true; })()")
        .await;
    c.ui.wait_for("[data-row=s4001][data-selected=true]", SECS)
        .await;
    assert!(
        in_view(c, "s4001").await,
        "the arrow keys keep the next row in view"
    );
    c.ui.view_shot("list-5000-far-row").await;
    c.ui.fill("#list-search", "s4999").await;
    c.ui.wait_for("[data-row=s4999]", SECS).await;
    assert_eq!(
        c.ui.count("[data-testid=list] [data-row]").await,
        1,
        "search shows only the match"
    );
    c.ui.fill("#list-search", "").await;
    c.ui.wait_for("[data-row=s4001][data-selected=true]", SECS)
        .await;
}

const TYPE: &str = r#"(async () => { const i = document.querySelector('#f-services-s0001-target'); const frame = () => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r))); const out = []; for (const ch of '90901') { const t = performance.now(); i.value = i.value + ch; i.dispatchEvent(new Event('input', { bubbles: true })); await frame(); out.push(Math.round(performance.now() - t)); } return JSON.stringify(out); })()"#;

async fn form_typing(c: &mut Ctx) {
    c.ui.nav("/services/s0001?tab=settings").await;
    edit_mode(c).await;
    c.ui.wait_for("#f-services-s0001-target", SECS).await;
    let ms: String = c.ui.eval(TYPE).await;
    let file = super::sheet::shots_dir().join("list-scale.txt");
    let old = std::fs::read_to_string(&file).unwrap_or_default();
    std::fs::write(
        &file,
        old + &format!("form typing at 5000, ms per key: {ms}\n"),
    )
    .expect("write the typing numbers");
    c.ui.click("[data-testid=discard-edit]").await;
    c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
}

fn at_5000(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        let got = measure(c, 5000).await;
        check(5000, &got);
        far_rows(c).await;
        form_typing(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn the_services_list_stays_smooth_with_500_services() {
    run("list-scale-500", opts(500), false, at_500);
}

#[test]
fn the_services_list_stays_smooth_with_5000_services() {
    run("list-scale-5000", opts(5000), false, at_5000);
}
