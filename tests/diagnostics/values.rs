use burrow::config::{Format, Role, load_str};

use super::visitors::visitors_server;
use super::{SECRET, env, render, server};

#[test]
fn reach_takes_any_or_rules_and_warns_only_for_any() {
    let codes = |reach: &str| {
        let src = visitors_server(
            &format!("  laptop:\n    secret: ${{HOME_SECRET}}\n    reach: {reach}\n"),
            "  db: { client: home, target: 5432, visitors: [laptop] }\n",
        );
        let o = load_str("server.yaml", &src, Format::Yaml, None, &env);
        let found: Vec<(&str, bool)> = o
            .report
            .diagnostics
            .iter()
            .filter(|d| d.code.contains("reach") || d.code.starts_with("invalid"))
            .map(|d| (d.code, d.is_error()))
            .collect();
        (found, o.loaded.is_some())
    };
    assert_eq!(codes("{ home: any }"), (vec![("reach-any", false)], true));
    assert_eq!(codes("{ home: 192.168.150.0/25 }"), (vec![], true));
    assert_eq!(
        codes("{ home: [10.0.0.0/8:22, 192.168.1.0/24:8000-8100, unix:/run/app.sock] }"),
        (vec![], true)
    );
    assert_eq!(
        codes("{ home: [any, 10.0.0.0/8] }"),
        (vec![("invalid-reach", true)], false)
    );
    assert_eq!(
        codes("{ home: [] }"),
        (vec![("invalid-reach", true)], false)
    );
    assert_eq!(
        codes("{ home: [nas.lan] }"),
        (vec![("invalid-cidr", true)], false)
    );
    assert_eq!(
        codes("{ home: [unix:run] }"),
        (vec![("invalid-socket", true)], false)
    );
}

#[test]
fn keepalive_longer_than_systems_accept_is_refused() {
    let client = |secs: u32| {
        format!(
            "server: example.com\nname: home\nsecret: {SECRET}\nallow: [127.0.0.0/8]\ntcp: {{ keepalive_secs: {secs} }}\n"
        )
    };
    let ok = load_str("client.yaml", &client(32767), Format::Yaml, None, &env);
    assert!(!ok.report.has_errors());
    let too_long = load_str("client.yaml", &client(100_000), Format::Yaml, None, &env);
    assert!(too_long.report.has_errors());
    assert!(
        too_long
            .report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("keepalive_secs")),
        "{:?}",
        too_long.report.diagnostics
    );
}

#[test]
fn a_config_of_another_role_is_one_clear_error() {
    let server =
        server("  web:\n    client: home\n    bind: 0.0.0.0:4000\n    target: 127.0.0.1:3000\n");
    for role in [Role::Client, Role::Visitor] {
        let o = load_str("server.yaml", &server, Format::Yaml, Some(role), &env);
        let errors: Vec<_> = o
            .report
            .diagnostics
            .iter()
            .filter(|d| d.is_error())
            .collect();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].code, burrow_admin_proto::codes::WRONG_ROLE);
        assert!(
            errors[0].message.contains("is a server config, not a"),
            "{}",
            errors[0].message
        );
        assert!(
            errors[0]
                .help
                .as_deref()
                .unwrap()
                .contains("server -c server.yaml")
        );
    }
    let client =
        format!("server: example.com\nname: home\nsecret: {SECRET}\nallow: [127.0.0.0/8]\n");
    let o = load_str(
        "client.yaml",
        &client,
        Format::Yaml,
        Some(Role::Server),
        &env,
    );
    assert!(
        o.report
            .diagnostics
            .iter()
            .any(|d| d.code == burrow_admin_proto::codes::WRONG_ROLE)
    );
    let o = load_str(
        "visitor.yaml",
        &client.replace("allow: [127.0.0.0/8]\n", ""),
        Format::Yaml,
        Some(Role::Visitor),
        &env,
    );
    assert!(
        !o.report
            .diagnostics
            .iter()
            .any(|d| d.code == burrow_admin_proto::codes::WRONG_ROLE),
        "a visitor without forwards looks like a client and is not refused"
    );
    let visitor = format!(
        "server: example.com\nname: laptop\nsecret: {SECRET}\nforwards:\n  db: {{ service: db, local: 5432 }}\n"
    );
    let o = load_str(
        "visitor.yaml",
        &visitor,
        Format::Yaml,
        Some(Role::Client),
        &env,
    );
    assert!(
        o.report
            .diagnostics
            .iter()
            .any(|d| d.code == burrow_admin_proto::codes::WRONG_ROLE)
    );
}

#[test]
fn a_bad_item_late_in_a_long_list_is_reported_where_it_is() {
    let many: String = (0..70)
        .map(|i| format!("      - 10.{i}.0.0/16\n"))
        .collect();
    let src = server(&format!(
        "  web:\n    client: home\n    bind: 0.0.0.0:4000\n    target: 127.0.0.1:3000\n    allow_from:\n{many}      - [x]\n"
    ));
    let o = load_str("server.yaml", &src, Format::Yaml, None, &env);
    let errors: Vec<_> = o
        .report
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .collect();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].message.contains("\"70\""),
        "{}",
        errors[0].message
    );
    assert!(!errors[0].message.contains("could not be loaded"));
}

#[test]
fn variables_fill_number_and_boolean_fields() {
    let src = format!(
        "server: example.com\nname: home\nsecret: {SECRET}\nallow: [127.0.0.0/8]\nconnect_timeout_secs: ${{T}}\ntcp:\n  keepalive_secs: ${{K}}\n  nodelay: ${{ND}}\n"
    );
    let vars = |t: &'static str, k: &'static str, nd: &'static str| {
        move |name: &str| match name {
            "T" => Some(t.to_string()),
            "K" => Some(k.to_string()),
            "ND" => Some(nd.to_string()),
            _ => None,
        }
    };
    let o = load_str(
        "client.yaml",
        &src,
        Format::Yaml,
        None,
        &vars("7", "45", "false"),
    );
    assert!(!o.report.has_errors(), "{:?}", o.report.diagnostics);
    let Some(burrow::config::Loaded::Client(c, _)) = o.loaded else {
        panic!("loads")
    };
    assert_eq!(c.connect_timeout, std::time::Duration::from_secs(7));
    assert_eq!(c.tcp.keepalive, Some(std::time::Duration::from_secs(45)));
    assert!(!c.tcp.nodelay);
    let bad = load_str(
        "client.yaml",
        &src,
        Format::Yaml,
        None,
        &vars("soon", "45", "maybe"),
    );
    let codes: Vec<&str> = bad
        .report
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| d.code)
        .collect();
    assert_eq!(codes.len(), 2, "{codes:?}");
    let far = load_str(
        "client.yaml",
        &src,
        Format::Yaml,
        None,
        &vars("7", "100000", "true"),
    );
    assert!(far.report.has_errors(), "the range still applies");
    let unset = load_str("client.yaml", &src, Format::Yaml, None, &|_: &str| None);
    let errors: Vec<&str> = unset
        .report
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| d.code)
        .collect();
    assert_eq!(
        errors,
        [burrow_admin_proto::codes::ENV_UNSET; 3],
        "one error per unset variable"
    );
    let name = load_str(
        "client.yaml",
        &format!("server: example.com\nname: ${{N}}\nsecret: {SECRET}\nallow: [127.0.0.0/8]\n"),
        Format::Yaml,
        None,
        &|_: &str| Some("123".to_string()),
    );
    assert!(
        !name.report.has_errors(),
        "a string field stays a string: {:?}",
        name.report.diagnostics
    );
    let literal = load_str(
        "client.yaml",
        &format!(
            "server: example.com\nname: home\nsecret: {SECRET}\nallow: [127.0.0.0/8]\nproxy: http://p:3128/$${{x}}\n"
        ),
        Format::Yaml,
        None,
        &|_: &str| None,
    );
    assert!(
        !literal
            .report
            .diagnostics
            .iter()
            .any(|d| d.code == burrow_admin_proto::codes::ENV_UNSET)
    );
}

#[test]
fn the_hint_for_an_unset_variable_fits_the_field_and_the_shell() {
    let src = "listen: 0.0.0.0:7835\nclients:\n  home:\n    secret: ${S}\nservices:\n  web:\n    client: home\n    bind: 8080\n    target: ${T}\n";
    let o = load_str("server.yaml", src, Format::Yaml, None, &|_: &str| None);
    let help = |var: &str| {
        o.report
            .diagnostics
            .iter()
            .find(|d| d.message.contains(&format!("\"{var}\"")))
            .and_then(|d| d.help.clone())
            .unwrap()
    };
    let (secret, target) = (help("S"), help("T"));
    assert!(secret.contains("gensecret"), "{secret}");
    assert!(!target.contains("gensecret"), "{target}");
    let set = if cfg!(windows) {
        "$env:T = "
    } else {
        "export T="
    };
    assert!(target.contains(set), "{target}");
}

#[test]
fn invalid_forwards_are_not_called_missing() {
    let warned = |forwards: &str| {
        let src = format!("server: example.com\nname: laptop\nsecret: {SECRET}\n{forwards}");
        let o = load_str(
            "visitor.yaml",
            &src,
            Format::Yaml,
            Some(Role::Visitor),
            &env,
        );
        o.report
            .diagnostics
            .iter()
            .any(|d| d.code == burrow_admin_proto::codes::NO_FORWARDS)
    };
    assert!(warned(""));
    assert!(!warned("forwards: {}\n"));
    assert!(!warned("forwards:\n  a: { service: db, local: 0 }\n"));
    assert!(!warned("forwards:\n"));
    assert!(!warned("forwards:\n  a: { service: db, local: 5432 }\n"));
}

#[test]
fn small_messages_say_what_is_wrong() {
    let leaked = render(
        "client.yaml",
        &format!(
            "server: wss://bob:hunter2@host/b\nname: home\nsecret: {SECRET}\nallow: [127.0.0.0/8]\n"
        ),
        None,
    );
    assert!(!leaked.contains("hunter2"), "{leaked}");
    let big = render(
        "server.yaml",
        &server(
            "  web:\n    client: home\n    bind: 0.0.0.0:4000\n    target: 127.0.0.1:3000\n    max_connections: 18446744073709551615\n",
        ),
        None,
    );
    assert!(big.contains("too large"), "{big}");
}

#[test]
fn an_unquoted_variable_in_a_flow_mapping_says_to_quote_it() {
    let shown = render(
        "server.yaml",
        "listen: 7835\nclients:\n  home: { secret: ${S} }\nservices: {}\n",
        None,
    );
    assert!(shown.contains("must be quoted, like \"${VAR}\""), "{shown}");
    for unrelated in [
        "listen: 7835\nclients:\n  home:\n    secret: \"${S}\" x\nservices: {}\n",
        "server: h.example.com:${PORT}: x\nname: a\n",
    ] {
        let shown = render("server.yaml", unrelated, None);
        assert!(!shown.contains("must be quoted"), "{shown}");
    }
}
