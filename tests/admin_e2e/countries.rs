use super::edits::Flow;
use super::world::{self, Options};
use super::{Ctx, SECS, both_widths, edit_mode, open, open_more, run};

const FIELD: &str = "[data-schema='services/*/allow_countries']";

async fn the_country_database_card(c: &mut Ctx) {
    c.ui.nav("/settings").await;
    let line =
        c.ui.wait_text(
            "[data-card=geoip] [data-testid=geoip-line]",
            "no database yet",
            SECS,
        )
        .await;
    assert!(
        line.contains("own.mmdb") && line.ends_with("file not found"),
        "plain words for the file: {line}"
    );
    assert_eq!(
        c.ui.count("[data-testid=country-db]").await,
        1,
        "one card holds the setting and its status"
    );
    let closed = c.ui.text("[data-testid=geoip-not-ready]").await;
    assert!(closed.contains("refuse public visitors"), "{closed}");
    let shop = c.ui.text("[data-testid=geoip-service]").await;
    assert_eq!(shop, "shop: allowed countries Belgium, France · 0 refused");
    assert_eq!(
        c.ui.count("[data-testid=geoip-credit]").await,
        0,
        "an own file is not DB-IP's"
    );
    both_widths(c, "countries-card").await;
}

async fn add(c: &Ctx, typed: &str) {
    let input = format!("{FIELD} input[list]");
    c.ui.fill(&input, typed).await;
    c.ui.press(&input, "Enter").await;
}

async fn pick_countries(c: &mut Ctx) {
    c.ui.nav("/services/shop?tab=settings").await;
    edit_mode(c).await;
    open_more(c, "services/*/allow_countries").await;
    let fr =
        c.ui.wait_text(&format!("{FIELD} [data-chip=FR]"), "France", SECS)
            .await;
    assert!(fr.contains("FR"), "{fr}");
    let options =
        c.ui.count(&format!("{FIELD} datalist option[value=CH]"))
            .await;
    assert_eq!(
        options, 1,
        "the picker offers every country by code and name"
    );
    for (typed, code, name) in [
        ("switzerland", "CH", "Switzerland"),
        ("United Kingdom", "GB", "United Kingdom"),
        ("de", "DE", "Germany"),
    ] {
        add(c, typed).await;
        let chip =
            c.ui.wait_text(&format!("{FIELD} [data-chip={code}]"), name, SECS)
                .await;
        assert!(chip.contains(code), "{typed} becomes {code}: {chip}");
    }
    add(c, "France").await;
    assert_eq!(
        c.ui.count(&format!("{FIELD} [data-chip]")).await,
        5,
        "a name already listed adds nothing"
    );
    add(c, "Atlantis").await;
    c.ui.wait_for(
        &format!("{FIELD} [data-chip=Atlantis][data-invalid=true]"),
        SECS,
    )
    .await;
    let why =
        c.ui.attr_of(&format!("{FIELD} [data-chip=Atlantis]"), "title")
            .await;
    assert!(why.contains("is not a country"), "{why}");
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
