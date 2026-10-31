use burrow::config::{Format, Role, load_str};

use super::{SECRET, env, render, server};

#[test]
fn client_valid() {
    insta::assert_snapshot!(render("client.yaml", &format!(
        "server: myserver.com:7835\nname: home\nsecret: {SECRET}\nallow:\n  - 127.0.0.0/8\n  - 192.168.1.0/24:22\n"
    ), None), @r#"client.yaml: valid client config for "home""#);
}

#[test]
fn client_bad_cidr() {
    insta::assert_snapshot!(render("client.yaml", &format!(
        "server: myserver.com:99999\nname: home\nsecret: {SECRET}\nallow:\n  - 10.0.0.0/33\n  - lan.local\n  - 192.168.1.1/24\n  - 10.0.0.0/8:http\n"
    ), None), @r#"
    error: invalid server address: "99999" is not a valid port (1-65535)
      --> client.yaml:1:9
       |
     1 | server: myserver.com:99999
       |         ^^^^^^^^^^^^^^^^^^ expected a host, host:port or wss://host/path
       = help: use a host name or IP (port 7835 unless you add :PORT), or the WebSocket URL (wss://tunnel.example.com/burrow)

    error: invalid allow entry: "33" is not a valid prefix length
      --> client.yaml:5:5
       |
     5 |   - 10.0.0.0/33
       |     ^^^^^^^^^^^ not a valid CIDR
       = help: use a CIDR like 192.168.1.0/24, optionally with a port (192.168.1.0/24:22) or a range (:8000-8100)

    error: invalid allow entry: "lan.local" is not an IP address or CIDR
      --> client.yaml:6:5
       |
     6 |   - lan.local
       |     ^^^^^^^^^ not a valid CIDR
       = help: allow entries are IP ranges, not host names; resolve the host and add its address, for example 192.168.1.10/32

    error: invalid allow entry: "192.168.1.1/24" has host bits set
      --> client.yaml:7:5
       |
     7 |   - 192.168.1.1/24
       |     ^^^^^^^^^^^^^^ not a valid CIDR
       = help: use a CIDR like 192.168.1.0/24, optionally with a port (192.168.1.0/24:22) or a range (:8000-8100)

    error: invalid allow entry: "http" is not a valid port or port range
      --> client.yaml:8:5
       |
     8 |   - 10.0.0.0/8:http
       |     ^^^^^^^^^^^^^^^ not a valid CIDR
       = help: use a CIDR like 192.168.1.0/24, optionally with a port (192.168.1.0/24:22) or a range (:8000-8100)

    client.yaml: 5 errors, 0 warnings
    "#);
}

#[test]
fn client_no_allow() {
    insta::assert_snapshot!(render("client.yaml", &format!("server: myserver.com:7835\nname: \"bad name\"\nsecret: {SECRET}\n"), None), @r#"
    warning: no allow list: this client will refuse every target
      --> client.yaml
       = help: add the networks the server may reach, for example: allow: [127.0.0.0/8]

    error: invalid client name "bad name"
      --> client.yaml:2:7
       |
     2 | name: "bad name"
       |       ^^^^^^^^^^ not a valid name
       = help: use 1-64 letters, digits, "-", "_" or "."

    client.yaml: 1 error, 1 warning
    "#);
}

#[test]
fn client_allow_all() {
    insta::assert_snapshot!(render("client.yaml", &format!(
        "server: myserver.com:7835\nname: home\nsecret: {SECRET}\nallow_all: true\nallow: [127.0.0.0/8]\ncipher: aes\nproxy: ftp://x\n"
    ), None), @r#"
    warning: allow_all is enabled: the server can make this client connect anywhere, Unix sockets included
      --> client.yaml:4:12
       |
     4 | allow_all: true
       |            ^^^^ no restriction on targets
       = help: list only the networks you expose, for example allow: [127.0.0.0/8]

    warning: "allow" is ignored because allow_all is true
      --> client.yaml:5:1
       |
     5 | allow: [127.0.0.0/8]
       | ^^^^^
       = help: remove allow_all to enforce the list

    error: unknown cipher "aes" in client config
      --> client.yaml:6:9
       |
     6 | cipher: aes
       |         ^^^ not a valid cipher
       = help: did you mean "aes-gcm"? valid values: chachapoly, aes-gcm

    error: invalid proxy: proxy must start with http:// or socks5://
      --> client.yaml:7:8
       = help: use http://host:port or socks5://[user:pass@]host:port

    client.yaml: 2 errors, 2 warnings
    "#);
}

#[test]
fn json_multiple_errors() {
    insta::assert_snapshot!(render("server.json", &format!(
        "{{\n  \"listen\": \"0.0.0.0:7835\",\n  \"clients\": {{\"home\": {{\"secret\": \"{SECRET}\"}}}},\n  \"services\": {{\n    \"web\": {{\"client\": \"hom\", \"bnd\": 4000, \"target\": \"127.0.0.1:3000\"}},\n    \"dns\": {{\"client\": \"home\", \"bind\": \"5353\", \"target\": \"1.1.1.1:53\", \"protocol\": \"udpp\"}}\n  }},\n  \"limits\": {{\"max_handshakes\": -1}}\n}}\n"
    ), None), @r#"
    error: service "web" has no "bind" and no "visitors", so nobody can reach it
      --> server.json:5:5
       |
     5 |     "web": {"client": "hom", "bnd": 4000, "target": "127.0.0.1:3000"},
       |     ^^^^^ no bind and no visitors
       = help: add "bind: 7840" to expose it publicly, or "visitors: [NAME]" for visitors using burrow visit

    error: unknown client "hom" referenced by service "web"
      --> server.json:5:23
       |
     5 |     "web": {"client": "hom", "bnd": 4000, "target": "127.0.0.1:3000"},
       |                       ^^^^^ no client with this name
       = help: did you mean "home"? defined clients: home

    error: unknown field "bnd" in service "web"
      --> server.json:5:30
       |
     5 |     "web": {"client": "hom", "bnd": 4000, "target": "127.0.0.1:3000"},
       |                              ^^^^^ unknown field
       = help: did you mean "bind"? valid fields: client, clients, mode, health_check, bind, target, protocol, max_connections, visitors, allow_from, deny_from, allow_countries, deny_countries, proxy_protocol, bandwidth, web

    error: unknown protocol "udpp" in service "dns"
      --> server.json:6:83
       |
     6 |     "dns": {"client": "home", "bind": "5353", "target": "1.1.1.1:53", "protocol": "udpp"}
       |                                                                                   ^^^^^^ not a valid protocol
       = help: did you mean "udp"? valid values: tcp, udp

    error: "max_handshakes" in "limits" must be between 1 and 1000000, found -1
      --> server.json:8:32
       |
     8 |   "limits": {"max_handshakes": -1}
       |                                ^^ must be between 1 and 1000000
       = help: use a value between 1 and 1000000

    server.json: 5 errors, 0 warnings
    "#);
}

#[test]
fn json_syntax_error() {
    insta::assert_snapshot!(render("client.json", "{\n  \"server\": \"a:1\",\n  \"name\": \"home\"\n  \"secret\": \"x\"\n}\n", None), @"
    error: invalid JSON: expected `,` or `}`
      --> client.json:4:3
       = help: check for a missing comma, quote or bracket just before this point

    client.json: 1 error, 0 warnings
    ");
}

#[test]
fn json_secret_line_hidden() {
    insta::assert_snapshot!(render("client.json", &format!("{{\"server\": \"a:1\", \"name\": \"home\", \"secret\": \"{SECRET}\", \"alow\": []}}"), None), @r#"
    warning: no allow list: this client will refuse every target
      --> client.json
       = help: add the networks the server may reach, for example: allow: [127.0.0.0/8]

    error: unknown field "alow" in client config
      --> client.json:1:93
       |
     1 | {"server": "a:1", "name": "home", "secret": **********************************************, "alow": []}
       |                                                                                             ^^^^^^ unknown field
       = help: did you mean "allow"? valid fields: server, name, secret, allow, allow_all, insecure_plaintext, headers, ca, cipher, proxy, reconnect, connect_timeout_secs, max_connections, max_udp_peers, tcp, udp

    client.json: 1 error, 1 warning
    "#);
}

#[test]
fn unknown_role() {
    insta::assert_snapshot!(render("x.yaml", "foo: 1\n", None), @r#"
    error: cannot tell whether this is a server, client or visitor config
      --> x.yaml
       = help: a server config has "listen" and "services", a client config has "server" and "name", a visitor config also has "forwards"; or pass --role server|client|visitor

    x.yaml: 1 error, 0 warnings
    "#);
}

#[test]
fn role_override_on_a_config_of_another_role_says_so() {
    insta::assert_snapshot!(render("server.yaml", "server: a:1\nname: home\n", Some(Role::Server)), @r#"
    error: server.yaml is a client config, not a server config
      --> server.yaml
       = help: run it with: burrow client -c server.yaml

    server.yaml: 1 error, 0 warnings
    "#);
}

#[test]
fn diagnostics_are_sorted_by_position() {
    let src = server(
        "  web:\n    client: hom\n    bnd: 1\n    target: x\n  zed:\n    client: nope\n    bind: 2\n    target: 127.0.0.1:1\n",
    );
    let o = load_str("s.yaml", &src, Format::Yaml, None, &env);
    let lines: Vec<usize> = o
        .report
        .diagnostics
        .iter()
        .filter_map(|d| d.span)
        .map(|s| s.start.line)
        .collect();
    let mut sorted = lines.clone();
    sorted.sort();
    assert_eq!(lines, sorted);
    assert!(
        o.report.error_count() >= 4,
        "all errors reported in one run"
    );
}

#[test]
fn yaml_and_json_report_the_same_problems() {
    let yaml = "listen: 0.0.0.0:7835\nclients:\n  home:\n    secret: x\nservices:\n  web:\n    client: hom\n    bnd: 1\n    target: a:1\n";
    let json = r#"{"listen": "0.0.0.0:7835", "clients": {"home": {"secret": "x"}}, "services": {"web": {"client": "hom", "bnd": 1, "target": "a:1"}}}"#;
    let codes = |name: &str, src: &str, f| {
        let mut c: Vec<_> = load_str(name, src, f, None, &env)
            .report
            .diagnostics
            .iter()
            .map(|d| d.code)
            .collect();
        c.sort();
        c
    };
    assert_eq!(
        codes("s.yaml", yaml, Format::Yaml),
        codes("s.json", json, Format::Json)
    );
}

#[test]
fn json_output_shape() {
    let o = load_str(
        "s.yaml",
        &server("  web:\n    client: hom\n    bind: 1\n    target: a:1\n"),
        Format::Yaml,
        None,
        &env,
    );
    let v = o.report.render_json();
    assert_eq!(v["valid"], false);
    let d = &v["diagnostics"][0];
    assert_eq!(d["code"], "unknown-client");
    assert_eq!(d["line"], 9);
    assert_eq!(d["column"], 13);
    assert!(
        d["help"]
            .as_str()
            .unwrap()
            .contains("did you mean \"home\"")
    );
}

#[test]
fn the_visitor_udp_options_speak_of_forward_ports() {
    let s = burrow::config::load::schema_json(Role::Visitor).to_string();
    assert!(s.contains("each port of a UDP forward"), "{s}");
    assert!(!s.contains("service port"), "{s}");
    assert!(s.contains("1000000"), "max_peers keeps its range: {s}");
}

#[test]
fn schema_matches_model_fields() {
    let s = burrow::config::load::schema_json(Role::Server);
    let props: Vec<&str> = s["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    for f in [
        "listen",
        "clients",
        "services",
        "insecure_plaintext",
        "heartbeat",
        "limits",
        "pool",
        "tcp",
        "udp",
    ] {
        assert!(props.contains(&f), "{f} missing from schema");
    }
    assert_eq!(s["additionalProperties"], false);
    let c = burrow::config::load::schema_json(Role::Client);
    assert_eq!(c["additionalProperties"], false);
}
