use burrow::base::style::Style;
use burrow::config::{Format, Role, load_str};

use super::{SECRET, env, render, server};

#[test]
fn websocket_server_errors() {
    let src = format!(
        "websocket:\n  listen: 0.0.0.0:7835\n  path: burrow\n  tls:\n    cert: missing-cert.pem\n    key: missing-key.pem\nlisten: 0.0.0.0:7835\nclients:\n  home:\n    secret: {SECRET}\nservices:\n  web:\n    client: home\n    bind: 7835\n    target: 127.0.0.1:3000\n"
    );
    insta::assert_snapshot!(render("server.yaml", &src, None), @r##"
    error: websocket.listen 0.0.0.0:7835 is the same address as listen
      --> server.yaml:2:11
       |
     2 |   listen: 0.0.0.0:7835
       |           ^^^^^^^^^^^^ already used by "listen"
       = help: use a different port for the websocket listener

    error: invalid websocket path "burrow"
      --> server.yaml:3:9
       |
     3 |   path: burrow
       |         ^^^^^^ expected a path like /burrow
       = help: start the path with "/" and use letters, digits and - . _ ~ / only, at most 1024 of them

    error: cannot read websocket.tls.cert "missing-cert.pem": file not found
      --> server.yaml:5:11
       |
     5 |     cert: missing-cert.pem
       |           ^^^^^^^^^^^^^^^^ file not readable
       = help: paths are relative to the config file; check the path and its permissions

    error: cannot read websocket.tls.key "missing-key.pem": file not found
      --> server.yaml:6:10
       |
     6 |     key: missing-key.pem
       |          ^^^^^^^^^^^^^^^ file not readable
       = help: paths are relative to the config file; check the path and its permissions

    error: service "web" binds 0.0.0.0:7835, which is the control port
      --> server.yaml:14:11
       |
    14 |     bind: 7835
       |           ^^^^ same as "listen"
       = help: pick a port other than 7835

    server.yaml: 5 errors, 0 warnings
    "##);
}

#[test]
fn server_without_any_listener() {
    let src = format!("clients:\n  home:\n    secret: {SECRET}\nservices: {{}}\n");
    insta::assert_snapshot!(render("server.yaml", &src, None), @r#"
    error: the server needs "listen" (TCP), "websocket", or both
      --> server.yaml
       = help: add "listen: 0.0.0.0:7835" for TCP clients, or a websocket section for clients behind HTTP-only networks (see https://github.com/charlesng35/burrow/blob/main/docs/websocket.md)

    warning: no services are defined, so nothing is exposed
      --> server.yaml:4:1
       |
     4 | services: {}
       | ^^^^^^^^
       = help: add a service, for example:
             services:
               web: { client: NAME, bind: 0.0.0.0:7840, target: 127.0.0.1:80 }

    server.yaml: 1 error, 1 warning
    "#);
}

#[test]
fn websocket_client_errors() {
    let src = format!(
        "server: https://tunnel.example.com/burrow\nname: home\nsecret: {SECRET}\nallow: [127.0.0.0/8]\nheaders:\n  Host: evil\n  X-Token: ok\nca: missing-ca.pem\n"
    );
    insta::assert_snapshot!(render("client.yaml", &src, None), @r#"
    error: invalid server address: "https://" is not supported; use host:port for TCP, or ws:// or wss:// for WebSocket
      --> client.yaml:1:9
       |
     1 | server: https://tunnel.example.com/burrow
       |         ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ expected a host, host:port or wss://host/path
       = help: use a host name or IP (port 7835 unless you add :PORT), or the WebSocket URL (wss://tunnel.example.com/burrow)

    error: header "Host" cannot be set
      --> client.yaml:6:3
       |
     6 |   Host: ****
       |   ^^^^ set by burrow itself
       = help: use custom headers such as an access token, for example CF-Access-Client-Id

    error: cannot use ca file "missing-ca.pem": file not found
      --> client.yaml:8:5
       |
     8 | ca: missing-ca.pem
       |     ^^^^^^^^^^^^^^ not a readable PEM certificate
       = help: paths are relative to the config file

    client.yaml: 3 errors, 0 warnings
    "#);
}

#[test]
fn websocket_client_warnings() {
    let src = format!(
        "server: myserver.com:7835\nname: home\nsecret: {SECRET}\nallow: [127.0.0.0/8]\nheaders:\n  X-Token: ok\n"
    );
    insta::assert_snapshot!(render("client.yaml", &src, None), @r#"
    warning: "headers" are only sent over WebSocket and are ignored for a TCP server address
      --> client.yaml:5:1
       |
     5 | headers:
       | ^^^^^^^
       = help: remove "headers", or use a ws:// or wss:// server address

    client.yaml: valid client config for "home", 1 warning
    "#);
    let ws = format!(
        "server: wss://tunnel.example.com/burrow\nname: home\nsecret: {SECRET}\nallow: [127.0.0.0/8]\ninsecure_plaintext: true\nreconnect:\n  on_rejected: exitt\n"
    );
    insta::assert_snapshot!(render("client.yaml", &ws, None), @r#"
    warning: insecure_plaintext does not apply over WebSocket: forwarded data stays encrypted
      --> client.yaml:5:21
       |
     5 | insecure_plaintext: true
       |                     ^^^^
       = help: it must still match the server setting; remove it on both sides to silence this

    error: unknown on_rejected "exitt" in "reconnect"
      --> client.yaml:7:16
       |
     7 |   on_rejected: exitt
       |                ^^^^^ not a valid on_rejected
       = help: did you mean "exit"? valid values: retry, exit

    client.yaml: 1 error, 1 warning
    "#);
}

#[test]
fn bare_ports_are_accepted() {
    use burrow::config::Loaded;
    for (listen, bind, target) in [
        ("7835", "7840", "3000"),
        ("\"7835\"", "\"7840\"", "\"3000\""),
        ("0.0.0.0:7835", "0.0.0.0:7840", "127.0.0.1:3000"),
    ] {
        let src = format!(
            "listen: {listen}\nclients:\n  home:\n    secret: {SECRET}\nservices:\n  web:\n    client: home\n    bind: {bind}\n    target: {target}\n"
        );
        let o = load_str("server.yaml", &src, Format::Yaml, None, &env);
        assert!(
            !o.report.has_errors(),
            "{}",
            o.report.render_text(Style { color: false })
        );
        let Some(Loaded::Server(s, _)) = o.loaded else {
            panic!("not a server")
        };
        assert_eq!(s.listen, Some("0.0.0.0:7835".parse().unwrap()));
        assert_eq!(
            s.services["web"].bind,
            Some("0.0.0.0:7840".parse().unwrap())
        );
        assert_eq!(s.services["web"].target, "127.0.0.1:3000");
    }
}

#[test]
fn client_server_without_port_uses_7835() {
    use burrow::config::Loaded;
    let src = format!("server: myserver.com\nname: home\nsecret: {SECRET}\nallow: [127.0.0.0/8]\n");
    let o = load_str("client.yaml", &src, Format::Yaml, None, &env);
    let Some(Loaded::Client(c, _)) = o.loaded else {
        panic!("{}", o.report.render_text(Style { color: false }))
    };
    assert_eq!(c.endpoint.dial_address(), "myserver.com:7835");
    assert_eq!(c.server, "myserver.com:7835");
}

#[test]
fn bad_listen_types_are_explained() {
    insta::assert_snapshot!(render("server.yaml", &format!("listen: [1]\nclients:\n  home:\n    secret: {SECRET}\nservices: {{}}\n"), None), @r#"
    error: "listen" in server config must be a whole number or a string, found a list
      --> server.yaml:1:9
       |
     1 | listen: [1]
       |         ^^^ expected a whole number or a string
       = help: use a single value, not a list

    warning: no services are defined, so nothing is exposed
      --> server.yaml:5:1
       |
     5 | services: {}
       | ^^^^^^^^
       = help: add a service, for example:
             services:
               web: { client: NAME, bind: 0.0.0.0:7840, target: 127.0.0.1:80 }

    server.yaml: 1 error, 1 warning
    "#);
}

#[test]
fn public_address_is_checked() {
    let with = |addr: &str| {
        format!(
            "listen: 0.0.0.0:7835\npublic_address: {addr}\nclients:\n  home:\n    secret: {SECRET}\nservices: {{web: {{client: home, bind: 7840, target: 3000}}}}\n"
        )
    };
    for ok in [
        "myserver.example.com",
        "myserver.example.com:9000",
        "203.0.113.5",
        "10.0.0.5",
        "\"[2001:db8::1]\"",
    ] {
        let text = render("server.yaml", &with(ok), None);
        assert!(
            text.ends_with("valid server config (1 client, 1 service)"),
            "{ok}: {text}"
        );
    }
    insta::assert_snapshot!(render("server.yaml", &with("0.0.0.0"), None), @"
    error: invalid public_address: 0.0.0.0 means every interface, not an address clients can connect to
      --> server.yaml:2:17
       |
     2 | public_address: 0.0.0.0
       |                 ^^^^^^^ expected a host, host:port or wss:// URL
       = help: use the host name or IP clients connect to, for example myserver.example.com

    server.yaml: 1 error, 0 warnings
    ");
    insta::assert_snapshot!(render("server.yaml", &with("wss://t.example.com/burrow"), None), @"
    error: invalid public_address: the server has no websocket section
      --> server.yaml:2:17
       |
     2 | public_address: wss://t.example.com/burrow
       |                 ^^^^^^^^^^^^^^^^^^^^^^^^^^ expected a host, host:port or wss:// URL
       = help: use the host name, or add a websocket section to the server config

    server.yaml: 1 error, 0 warnings
    ");
}

#[test]
fn syntax_errors_do_not_show_secrets() {
    for src in [
        format!(
            "listen: 7835\nclients:\n  home:\n    secret:\n      - \"{SECRET}\" x\nservices: {{}}\n"
        ),
        format!(
            "listen: 7835\nclients:\n  home:\n    secret:\n      \"{SECRET}\" x\nservices: {{}}\n"
        ),
        format!("listen: 7835\nclients: {{home: {{secret: \"{SECRET}\" x}}}}\n"),
    ] {
        let text = render("server.yaml", &src, None);
        assert!(text.contains("error: invalid syntax"), "{text}");
    }
    let text = render(
        "server.json",
        &format!(
            "{{\"listen\": \"7835\", \"clients\": {{\"home\": {{\"secret\": \"{SECRET}\" x}}}}}}"
        ),
        None,
    );
    assert!(text.contains("invalid JSON"), "{text}");
}

#[test]
fn header_values_and_proxy_passwords_are_hidden() {
    let src = format!(
        "{{\"server\": \"h\", \"name\": \"home\", \"secret\": \"{SECRET}\", \"allow\": [\"127.0.0.0/8\"], \"headers\": {{\"X Tok\": \"tok-hidden-value\"}}, \"proxy\": \"socks5://u:hunter2@proxy:1080\"}}"
    );
    let text = render("client.json", &src, None);
    assert!(text.contains("headers"), "{text}");
    for leak in ["tok-hidden-value", "hunter2"] {
        assert!(!text.contains(leak), "{leak} leaked:\n{text}");
    }
}

#[test]
fn a_missing_server_does_not_cause_warnings_about_it() {
    let text = render(
        "client.yaml",
        &format!("name: home\nsecret: {SECRET}\nallow: [127.0.0.0/8]\nheaders: {{X-A: b}}\n"),
        Some(Role::Client),
    );
    assert!(text.contains("missing field \"server\""), "{text}");
    assert!(!text.contains("ignored for a TCP server address"), "{text}");
}

#[test]
fn ipv4_and_ipv6_wildcards_on_one_port_conflict() {
    let text = render(
        "server.yaml",
        &server(
            "  a: {client: home, bind: \"0.0.0.0:17840\", target: 3000}\n  b: {client: home, bind: \"[::]:17840\", target: 3001}\n",
        ),
        None,
    );
    assert!(text.contains("error"), "{text}");
}

#[test]
fn a_byte_order_mark_is_ignored() {
    let yaml = format!(
        "\u{feff}{}",
        server("  web: {client: home, bind: 7840, target: 3000}\n")
    );
    let text = render("server.yaml", &yaml, None);
    assert!(
        text.ends_with("valid server config (2 clients, 1 service)"),
        "{text}"
    );
    let json = format!(
        "\u{feff}{{\"listen\": \"0.0.0.0:7835\", \"clients\": {{\"home\": {{\"secret\": \"{SECRET}\"}}}}}}"
    );
    assert!(
        render("server.json", &json, None).contains("valid server config (1 client, 0 services)")
    );
}

#[test]
fn targets_longer_than_the_wire_allows_are_rejected() {
    let host = format!("{}.example.com", "d".repeat(240));
    let text = render(
        "server.yaml",
        &server(&format!(
            "  web: {{client: home, bind: 7840, target: \"{host}:8080\"}}\n"
        )),
        None,
    );
    assert!(text.contains("is longer than 255 characters"), "{text}");
}

#[test]
fn websocket_listen_takes_a_bare_port_too() {
    use burrow::config::Loaded;
    let src = format!(
        "websocket:\n  listen: 7836\nclients:\n  home:\n    secret: {SECRET}\nservices:\n  web: {{client: home, bind: 7840, target: 3000}}\n"
    );
    let o = load_str("server.yaml", &src, Format::Yaml, None, &env);
    assert!(
        !o.report.has_errors(),
        "{}",
        o.report.render_text(Style { color: false })
    );
    let Some(Loaded::Server(s, _)) = o.loaded else {
        panic!("not a server")
    };
    assert_eq!(s.websocket.unwrap().listen, "0.0.0.0:7836".parse().unwrap());
}
