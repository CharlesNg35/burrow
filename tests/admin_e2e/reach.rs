use super::edits::{Flow, click_js};
use super::world::{self, Options};
use super::{Ctx, SECS, applied, both_widths, edit_mode, open, open_review, run, wait_checked};

const VIEW: &str = "[data-testid=reached-by-view]";

fn opts(port: u16) -> Options {
    Options {
        extra_top: "web:\n  https: 127.0.0.1:HTTPS_PORT\n".into(),
        extra_services: format!(
            "  nas:\n    client: home\n    bind: 127.0.0.1:{port}\n    target: 8443\n    visitors: [laptop]\n    web: {{ hosts: [nas.example.com], tls: passthrough }}\n  dns:\n    client: home\n    protocol: udp\n    target: 53\n    visitors: [laptop]\n"
        ),
        ..Options::default()
    }
}

async fn open_nas(c: &Ctx) {
    c.ui.nav("/services/nas").await;
    c.ui.wait_for("[role=tablist] [data-tab=settings]", SECS)
        .await;
    c.ui.click("[role=tablist] [data-tab=settings]").await;
    c.ui.wait_for("[data-testid=reached-by]", SECS).await;
}

async fn switch(c: &Ctx, way: &str) {
    click_js(c, &format!("#f-services-nas-reach-{way}")).await;
}

async fn all_three_show(c: &mut Ctx, port: u16) {
    open_nas(c).await;
    let lines = c.ui.wait_text(VIEW, "nas.example.com", SECS).await;
    assert!(
        lines.contains(&port.to_string()) && lines.contains("laptop"),
        "a hand-edited file with all three shows all three: {lines}"
    );
    for who in ["on its own port", "browsers, by name", "burrow visitor key"] {
        assert!(lines.contains(who), "{who} in {lines}");
    }
    let stacked: bool = c
        .ui
        .eval(&format!("[...document.querySelectorAll('{VIEW} [data-way]')].every(li => li.querySelector('[data-testid=way-value]').getBoundingClientRect().top >= li.firstElementChild.getBoundingClientRect().bottom)"))
        .await;
    assert!(stacked, "each way in shows its value under its label");
    both_widths(c, "reached-by-view").await;
    edit_mode(c).await;
    for way in ["bind", "web", "visitors"] {
        c.ui.wait_for(&format!("#f-services-nas-reach-{way}:checked"), SECS)
            .await;
    }
    c.ui.wait_for("[data-testid=edit-website]", SECS).await;
    both_widths(c, "reached-by-edit").await;
}

async fn off_then_on_keeps_the_values(c: &mut Ctx, port: u16) {
    switch(c, "bind").await;
    c.ui.wait_for("#f-services-nas-reach-bind:not(:checked)", SECS)
        .await;
    switch(c, "bind").await;
    let bind: String = c
        .ui
        .eval("(() => { const f = document.getElementById('f-services-nas-bind'); const i = f && (f.matches('input') ? f : f.querySelector('input')); return i ? i.value : ''; })()")
        .await;
    assert!(
        bind.contains(&port.to_string()),
        "the port comes back: {bind}"
    );
    switch(c, "visitors").await;
    switch(c, "visitors").await;
    c.ui.wait_for("[data-way=visitors] [data-chip=laptop]", SECS)
        .await;
    switch(c, "web").await;
    c.ui.wait_text("[data-testid=website-off]", "not a website", SECS)
        .await;
    both_widths(c, "website-off").await;
    switch(c, "web").await;
    c.ui.wait_text("[data-testid=website-summary]", "nas.example.com", SECS)
        .await;
    c.ui.click("[data-testid=discard-edit]").await;
    c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
}

async fn turn_off(c: &mut Ctx, way: &str, says: &str, step: &str) -> String {
    open_nas(c).await;
    edit_mode(c).await;
    c.ui.wait_for(&format!("#f-services-nas-reach-{way}:checked"), SECS)
        .await;
    switch(c, way).await;
    wait_checked(c).await;
    open_review(c, &format!("{step}-review")).await;
    let text = c.ui.wait_text("[data-testid=impacts]", says, SECS).await;
    if c.ui.count("[data-confirm=nas]").await > 0 {
        c.ui.fill("[data-confirm=nas]", "nas").await;
    }
    applied(c, step).await;
    text
}

async fn the_review_says_what_goes(c: &mut Ctx, port: u16) {
    let text = turn_off(c, "web", "stops website", "website-off-applied").await;
    assert!(
        text.contains(
            "stops website nas.example.com and removes its settings: HTTPS (the app does its own)"
        ),
        "{text}"
    );
    assert!(!c.world.config().contains("nas.example.com"));
    let text = turn_off(c, "visitors", "removes visitor", "visitors-off").await;
    assert!(
        text.contains("removes visitor laptop: it can no longer reach nas with burrow visit"),
        "{text}"
    );
    c.world.hand_edit(|t| {
        t.replace(
            &format!("    bind: 127.0.0.1:{port}\n"),
            &format!("    bind: 127.0.0.1:{port}\n    visitors: [laptop]\n"),
        )
    });
    open_nas(c).await;
    c.ui.wait_text(VIEW, "laptop", SECS).await;
    let text = turn_off(c, "bind", "closes port", "public-off").await;
    assert!(
        text.contains(&format!("closes port {port} (removes bind:")),
        "{text}"
    );
    assert!(!c.world.config().contains(&format!("127.0.0.1:{port}")));
    open_nas(c).await;
    edit_mode(c).await;
    switch(c, "visitors").await;
    wait_checked(c).await;
    assert!(
        !c.ui.enabled("[data-testid=save]").await,
        "with no way in the service cannot be saved"
    );
    both_widths(c, "no-way-in").await;
    c.ui.click("[data-testid=discard-edit]").await;
    c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
}

async fn udp_has_no_website(c: &mut Ctx) {
    c.ui.nav("/services/dns?tab=settings").await;
    edit_mode(c).await;
    let reason =
        c.ui.wait_text("[data-way=web] [data-testid=website-blocked]", "TCP", SECS)
            .await;
    assert!(reason.contains("websites need TCP"), "{reason}");
    assert!(!c.ui.enabled("#f-services-dns-reach-web").await);
    both_widths(c, "udp-no-website").await;
    c.ui.click("[data-testid=discard-edit]").await;
}

fn reached_by(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        let port: u16 = c
            .world
            .config()
            .lines()
            .find_map(|l| {
                l.trim()
                    .strip_prefix("bind: 127.0.0.1:")
                    .and_then(|p| p.parse().ok())
            })
            .expect("the nas port");
        open(c, "ops").await;
        all_three_show(c, port).await;
        off_then_on_keeps_the_values(c, port).await;
        udp_has_no_website(c).await;
        the_review_says_what_goes(c, port).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_service_is_reached_by_any_mix_of_public_port_website_and_visitors() {
    run("reached-by", opts(world::free_port()), false, reached_by);
}
