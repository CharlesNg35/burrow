use burrow::config::{Format, Role, load_str};

use super::visitors::visitors_server;
use super::{SECRET, SECRET2, env, render, server};

#[test]
fn a_connection_limit_on_a_udp_service_is_refused() {
    let shown = render(
        "server.yaml",
        &server(
            "  dns: { client: home, protocol: udp, bind: 5353, target: 53, max_connections: 10 }\n",
        ),
        None,
    );
    assert!(
        shown.contains(
            "max_connections only works for tcp services; for UDP, udp.max_peers limits the peers of each port"
        ),
        "{shown}"
    );
    assert!(!shown.contains("4096"), "{shown}");
    assert!(!shown.contains("valid server config"), "{shown}");
}

#[test]
fn a_connection_limit_of_zero_is_refused() {
    for src in [
        server("  web: { client: home, bind: 4000, target: 3000, max_connections: 0 }\n"),
        format!(
            "listen: 0.0.0.0:7835\nclients:\n  home: {{ secret: {SECRET}, max_connections: 0 }}\nservices: {{}}\n"
        ),
    ] {
        let shown = render("server.yaml", &src, None);
        assert!(shown.contains("must be at least 1"), "{shown}");
    }
}

#[test]
fn a_client_connection_limit_outside_1_to_1000000_is_refused_with_the_range_and_default() {
    let client = |v: &str| {
        render(
            "client.yaml",
            &format!(
                "server: example.com\nname: home\nsecret: {SECRET}\nallow: [127.0.0.0/8]\nmax_connections: {v}\n"
            ),
            None,
        )
    };
    for bad in ["0", "-1", "1000001", "lots", "1.5"] {
        let shown = client(bad);
        assert!(
            shown.contains("error: \"max_connections\" in client config must be"),
            "{shown}"
        );
        assert!(
            shown.contains("help: use a value between 1 and 1000000; the default is 16384"),
            "{shown}"
        );
    }
    assert!(client("0").contains("must be between 1 and 1000000, found 0"));
    for good in ["1", "1000000"] {
        assert!(
            client(good).contains("valid client config"),
            "{}",
            client(good)
        );
    }
}

#[test]
fn udp_peer_limits_outside_1_to_1000000_are_refused_with_the_range_and_default() {
    let client = |v: &str| {
        render(
            "client.yaml",
            &format!(
                "server: example.com\nname: home\nsecret: {SECRET}\nallow: [127.0.0.0/8]\nmax_udp_peers: {v}\n"
            ),
            None,
        )
    };
    let server = |v: &str| {
        render(
            "server.yaml",
            &format!("udp:\n  max_peers: {v}\n{}", server("  {}\n")),
            None,
        )
    };
    for bad in ["0", "-1", "1000001", "lots", "1.5"] {
        let shown = client(bad);
        assert!(
            shown.contains("error: \"max_udp_peers\" in client config must be"),
            "{shown}"
        );
        assert!(
            shown.contains("help: use a value between 1 and 1000000; the default is 8192"),
            "{shown}"
        );
        let shown = server(bad);
        assert!(
            shown.contains("error: \"max_peers\" in \"udp\" must be"),
            "{shown}"
        );
        assert!(
            shown.contains("help: use a value between 1 and 1000000; the default is 4096"),
            "{shown}"
        );
    }
    for good in ["1", "1000000"] {
        assert!(
            client(good).contains("valid client config"),
            "{}",
            client(good)
        );
        assert!(
            server(good).contains("valid server config"),
            "{}",
            server(good)
        );
    }
}

#[test]
fn a_handshake_limit_beyond_what_any_build_can_hold_is_refused() {
    let limit = |n: u64| {
        render(
            "server.yaml",
            &format!("limits:\n  max_handshakes: {n}\n{}", server("  {}\n")),
            None,
        )
    };
    assert!(
        limit(1_000_000).contains("valid server config"),
        "{}",
        limit(1_000_000)
    );
    let big = limit(1_000_000_000);
    assert!(
        big.contains("error") && big.contains("max_handshakes"),
        "{big}"
    );
}

#[test]
fn source_checks_for_visitors_behind_a_websocket_proxy_are_flagged() {
    let codes = |ws: &str, extra: &str| {
        let src = visitors_server(
            "  laptop:\n    secret: ${HOME_SECRET}\n",
            &format!("  db: {{ client: home, target: 5432, visitors: [laptop]{extra} }}\n"),
        )
        .replace(
            "listen: 7835\n",
            &format!("listen: 7835\nwebsocket:\n  listen: {ws}\n"),
        );
        let o = load_str("server.yaml", &src, Format::Yaml, None, &env);
        o.report
            .diagnostics
            .iter()
            .filter(|d| d.code == "proxied-source")
            .count()
    };
    assert_eq!(codes("127.0.0.1:7836", ", deny_from: [203.0.113.0/24]"), 1);
    assert_eq!(codes("127.0.0.1:7836", ", proxy_protocol: v2"), 1);
    assert_eq!(codes("127.0.0.1:7836", ""), 0);
    assert_eq!(codes("0.0.0.0:7837", ", allow_from: [10.0.0.0/8]"), 0);
}

#[test]
fn a_file_named_like_a_visitor_keeps_the_role_its_content_shows() {
    let dir = tempfile::tempdir().unwrap();
    let role_of = |name: &str, text: &str| {
        let path = dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        let o = burrow::config::load_file(&path, None, &env).unwrap();
        (o.role, o.report.has_errors())
    };
    assert_eq!(
        role_of(
            "relay-for-visitor.yaml",
            &server("  web: { client: home, bind: 4000, target: 3000 }\n")
        ),
        (Some(Role::Server), false)
    );
    let client = format!(
        "server: tunnel.example.com\nname: home\nsecret: {SECRET}\nallow: [127.0.0.1/32]\n"
    );
    assert_eq!(
        role_of("home-visitor.yaml", &client),
        (Some(Role::Client), false)
    );
    let visitor = format!("server: tunnel.example.com\nname: laptop\nsecret: {SECRET}\n");
    assert_eq!(role_of("visitor.yaml", &visitor).0, Some(Role::Visitor));
}

#[cfg(unix)]
#[test]
fn a_config_path_that_is_a_pipe_is_refused_instead_of_waited_on() {
    let dir = tempfile::tempdir().unwrap();
    let pipe = dir.path().join("server.yaml");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&pipe)
            .status()
            .unwrap()
            .success()
    );
    let Err(e) = burrow::config::load_file(&pipe, None, &env) else {
        panic!("a pipe loaded as a config")
    };
    assert!(format!("{e:?}").contains("not a regular file"), "{e:?}");
}

#[test]
fn a_config_with_old_mac_line_endings_keeps_its_secrets_hidden() {
    let src =
        server("  web:\n    client: home\n    bnd: 0.0.0.0:4000\n    target: 127.0.0.1:3000\n")
            .replace('\n', "\r");
    let text = render("server.yaml", &src, None);
    assert!(text.contains("unknown field \"bnd\""), "{text}");
}

#[test]
fn secrets_in_the_wrong_shape_or_under_a_wrong_key_stay_hidden() {
    let shapes = [
        format!("clients:\n  home: {SECRET}\n"),
        format!("clients:\n  - name: home\n    secret: {SECRET}\n"),
        format!("visitors:\n  lap: {SECRET}\n"),
        format!("clients:\n  home:\n    key: {SECRET}\n"),
        format!("clients:\n  home:\n    psk: {SECRET}\n"),
        format!("clients:\n  home:\n    client_secret: {SECRET}\n"),
        format!("clients:\n  home:\n    secret: {SECRET}\n    password: {SECRET2}\n"),
    ];
    for src in shapes {
        render("server.yaml", &format!("listen: 0.0.0.0:7835\n{src}"), None);
    }
    for src in [
        format!(
            "server: example.com:7835\nname: home\nsecret: {SECRET}\nheaders:\n  - \"Authorization: Bearer {SECRET2}\"\n"
        ),
        format!(
            "server: example.com:7835\nname: home\nsecret: {SECRET}\nheaders: \"Authorization: Bearer {SECRET2}\"\n"
        ),
        format!("server: example.com:7835\nname: home\ntoken: {SECRET}\n"),
        format!(
            "server: wss://alice:Zq9/{SECRET2}@example.com/burrow\nname: home\nsecret: {SECRET}\n"
        ),
    ] {
        render("client.yaml", &src, None);
    }
}
