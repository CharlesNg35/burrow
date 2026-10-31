use super::{render, server};

fn shop(rules: &str) -> String {
    server(&format!(
        "  shop:\n    client: home\n    bind: 8443\n    target: 443\n{rules}"
    ))
}

#[test]
fn country_codes_are_checked_and_only_one_rule_per_service() {
    let both = render(
        "server.yaml",
        &shop("    allow_countries: [FR]\n    deny_countries: [RU]\n"),
        None,
    );
    assert!(
        both.contains("error: service \"shop\" has both allow_countries and deny_countries"),
        "{both}"
    );
    let bad = render(
        "server.yaml",
        &shop("    allow_countries: [fr, France, ZZ]\n"),
        None,
    );
    assert!(
        bad.contains("use the two-letter code FR for France"),
        "{bad}"
    );
    assert!(
        bad.contains("\"ZZ\" is not a two-letter ISO 3166 country code"),
        "{bad}"
    );
    assert!(bad.contains("2 errors"), "lower case is fine: {bad}");
}

#[test]
fn validate_says_when_the_country_database_is_not_ready_or_not_used() {
    let fresh = render(
        "server.yaml",
        &shop("    allow_countries: [FR, BE]\n"),
        None,
    );
    assert!(
        fresh.contains("warning: the country database is not ready"),
        "{fresh}"
    );
    assert!(
        fresh.contains("until then, service \"shop\" refuses public visitors (allow_countries)"),
        "{fresh}"
    );
    let own = render(
        "server.yaml",
        &format!(
            "geoip: /nowhere/GeoLite2-Country.mmdb\n{}",
            shop("    deny_countries: [RU]\n")
        ),
        None,
    );
    assert!(
        own.contains("/nowhere/GeoLite2-Country.mmdb does not exist yet"),
        "{own}"
    );
    assert!(
        own.contains("deny_countries lets every visitor in"),
        "{own}"
    );
    let unused = render("server.yaml", &format!("geoip: db-ip\n{}", shop("")), None);
    assert!(
        unused.contains(
            "warning: geoip is set, but no service has allow_countries or deny_countries"
        ),
        "{unused}"
    );
    let url = render(
        "server.yaml",
        &format!(
            "geoip: https://example.com/x.mmdb\n{}",
            shop("    allow_countries: [FR]\n")
        ),
        None,
    );
    assert!(
        url.contains("error: geoip \"https://example.com/x.mmdb\" is a URL"),
        "{url}"
    );
    let private = render(
        "server.yaml",
        &server(
            "  db:\n    client: home\n    target: 5432\n    visitors: [laptop]\n    allow_countries: [FR]\nvisitors:\n  laptop:\n    secret: q4x7Z0m3VQ1n8kPp2rT6sW9yB5cD0eF3gH7jK1lM4nB=\n",
        ),
        None,
    );
    assert!(
        private.contains("allow_countries of service \"db\" has no effect"),
        "{private}"
    );
}
