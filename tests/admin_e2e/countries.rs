use super::edits::Flow;
use super::world::{self, Options};
use super::{Ctx, SECS, both_widths, edit_mode, open, open_more, run};

const FIELD: &str = "[data-schema='services/*/allow_countries']";

async fn the_country_database_card(c: &mut Ctx) {
    c.ui.nav("/settings").await;
    let line =
        c.ui.wait_text("[data-testid=geoip-line]", "no database yet", SECS)
            .await;
    assert!(line.contains("own.mmdb"), "the card names the file: {line}");
    let closed = c.ui.text("[data-testid=geoip-not-ready]").await;
    assert!(closed.contains("refuse public visitors"), "{closed}");
    let shop = c.ui.text("[data-testid=geoip-service]").await;
    assert!(
        shop.contains("shop: allow_countries BE, FR, 0 refused"),
        "{shop}"
    );
    assert_eq!(
        c.ui.count("[data-testid=geoip-credit]").await,
        0,
        "an own file is not DB-IP's"
    );
    both_widths(c, "countries-card").await;
}

async fn pick_countries(c: &mut Ctx) {
    c.ui.nav("/services/shop?tab=settings").await;
    edit_mode(c).await;
    open_more(c, "services/*/allow_countries").await;
    let fr =
        c.ui.wait_text(&format!("{FIELD} [data-chip=FR]"), "France", SECS)
            .await;
    assert!(fr.contains("FR"), "{fr}");
    let input = format!("{FIELD} input[list]");
    let options =
        c.ui.count(&format!("{FIELD} datalist option[value=CH]"))
            .await;
    assert_eq!(
        options, 1,
        "the picker offers every country by code and name"
    );
    c.ui.fill(&input, "ch").await;
    c.ui.press(&input, "Enter").await;
    let ch =
        c.ui.wait_text(&format!("{FIELD} [data-chip=CH]"), "Switzerland", SECS)
            .await;
    assert!(ch.contains("CH"), "a typed code becomes upper case: {ch}");
    c.ui.fill(&input, "France").await;
    c.ui.press(&input, "Enter").await;
    c.ui.wait_for(
        &format!("{FIELD} [data-chip=France][data-invalid=true]"),
        SECS,
    )
    .await;
    let why =
        c.ui.attr_of(&format!("{FIELD} [data-chip=France]"), "title")
            .await;
    assert!(
        why.contains("use the two-letter code FR for France"),
        "{why}"
    );
    both_widths(c, "countries-picker").await;
    c.ui.click("[data-testid=discard-edit]").await;
    c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
}

fn countries(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        the_country_database_card(c).await;
        pick_countries(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn countries_are_picked_by_code_and_the_database_shows_in_settings() {
    let opts = Options {
        extra_top: "geoip: /nowhere/own.mmdb\n".into(),
        extra_services: format!(
            "  shop:\n    client: home\n    bind: 127.0.0.1:{}\n    target: 8443\n    allow_countries: [FR, BE]\n",
            world::free_port()
        ),
        ..Options::default()
    };
    run("countries", opts, false, countries);
}
