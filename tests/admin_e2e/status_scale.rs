use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::network::{
    EnableParams as NetworkOn, EventWebSocketFrameReceived,
};
use chromiumoxide::cdp::browser_protocol::performance::{EnableParams, GetMetricsParams};
use futures_util::StreamExt;

use super::edits::Flow;
use super::{Ctx, SECS, open, run};

const SERVICES: usize = 5000;
const WINDOW: Duration = Duration::from_secs(10);

async fn busy_ms(c: &Ctx) -> (f64, f64) {
    let m =
        c.ui.page
            .execute(GetMetricsParams::default())
            .await
            .expect("the page's performance metrics");
    let get = |n: &str| {
        m.result
            .metrics
            .iter()
            .find(|x| x.name == n)
            .map_or(0.0, |x| x.value * 1000.0)
    };
    (get("TaskDuration"), get("ScriptDuration"))
}

async fn per_tick(c: &Ctx, page: &str) -> String {
    c.ui.nav(page).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let (t0, s0) = busy_ms(c).await;
    tokio::time::sleep(WINDOW).await;
    let (t1, s1) = busy_ms(c).await;
    let secs = WINDOW.as_secs_f64();
    format!(
        "{page}: main thread {:.1} ms/s, script {:.1} ms/s",
        (t1 - t0) / secs,
        (s1 - s0) / secs
    )
}

async fn wire(c: &Ctx) -> (u64, u64) {
    c.ui.page
        .execute(NetworkOn::default())
        .await
        .expect("network events");
    let mut frames =
        c.ui.page
            .event_listener::<EventWebSocketFrameReceived>()
            .await
            .expect("websocket frames");
    let (count, bytes) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
    let (n, b) = (count.clone(), bytes.clone());
    let task = tokio::spawn(async move {
        while let Some(f) = frames.next().await {
            if f.response.payload_data.contains("\"ev\":\"status") {
                n.fetch_add(1, Ordering::Relaxed);
                b.fetch_add(f.response.payload_data.len() as u64, Ordering::Relaxed);
            }
        }
    });
    tokio::time::sleep(WINDOW).await;
    task.abort();
    (count.load(Ordering::Relaxed), bytes.load(Ordering::Relaxed))
}

fn at_5000(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.wait_for("[data-testid=overview]", SECS).await;
        c.ui.page
            .execute(EnableParams::default())
            .await
            .expect("performance metrics on");
        let mut note = format!("status-scale {SERVICES} services:");
        for page in ["/", "/services", "/about"] {
            note.push_str(&format!(" | {}", per_tick(c, page).await));
        }
        c.ui.nav("/").await;
        let (count, bytes) = wire(c).await;
        let secs = WINDOW.as_secs();
        note.push_str(&format!(
            " | status on the wire: {count} messages, {} B per message, {} B/s\n",
            bytes / count.max(1),
            bytes / secs
        ));
        println!("{note}");
        let file = super::sheet::shots_dir().join("status-scale.txt");
        let old = std::fs::read_to_string(&file).unwrap_or_default();
        std::fs::write(&file, old + &note).expect("write the status numbers");
        assert!(
            count >= secs - 2,
            "the status keeps coming once a second at {SERVICES} services: {note}"
        );
        c.ui.assert_no_errors();
    })
}

#[test]
fn live_status_costs_little_per_second_with_5000_services() {
    run(
        "status-scale-5000",
        super::list_scale::opts(SERVICES),
        false,
        at_5000,
    );
}
