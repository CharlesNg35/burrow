use burrow::base::style::Style;
use burrow::config::{Format, Role, load_str};

const SECRET: &str = "yjgGDKUL5TY/9SCE2jvBsr2z5zLSKl+x1p76T98Vf/U=";
const SECRET2: &str = "q4x7Z0m3VQ1n8kPp2rT6sW9yB5cD0eF3gH7jK1lM4nA=";

fn env(name: &str) -> Option<String> {
    match name {
        "HOME_SECRET" => Some(SECRET2.to_string()),
        _ => None,
    }
}

fn render(name: &str, src: &str, role: Option<Role>) -> String {
    let format = if name.ends_with(".json") {
        Format::Json
    } else {
        Format::Yaml
    };
    let o = load_str(name, src, format, role, &env);
    let mut text = o.report.render_text(Style { color: false });
    text.push_str(&burrow::config::validate::summary_line(&o, false));
    for s in [SECRET, SECRET2] {
        assert!(!text.contains(s), "secret leaked in diagnostics:\n{text}");
    }
    let json = serde_json::to_string(&o.report.render_json()).unwrap();
    for s in [SECRET, SECRET2] {
        assert!(!json.contains(s), "secret leaked in json output");
    }
    text
}

fn server(extra_service: &str) -> String {
    format!(
        "listen: 0.0.0.0:7835\nclients:\n  home:\n    secret: {SECRET}\n  office:\n    secret: ${{HOME_SECRET}}\nservices:\n{extra_service}"
    )
}

#[test]
fn a_visitor_listed_twice_is_an_error_at_the_second_mention() {
    let src = format!(
        "listen: 0.0.0.0:7835\nclients:\n  home:\n    secret: {SECRET}\nvisitors:\n  lap:\n    secret: ${{HOME_SECRET}}\nservices:\n  db:\n    client: home\n    target: 5432\n    visitors:\n      - lap\n      - lap\n"
    );
    let text = render("server.yaml", &src, None);
    assert!(
        text.contains("error: service \"db\" lists visitor \"lap\" twice"),
        "{text}"
    );
    assert!(text.contains("--> server.yaml:14:9"), "{text}");
    assert!(text.contains("list each visitor once"), "{text}");
}

#[test]
fn valid_server() {
    insta::assert_snapshot!(render("server.yaml", &server("  web:\n    client: home\n    bind: 0.0.0.0:4000\n    target: 127.0.0.1:3000\n"), None), @"server.yaml: valid server config (2 clients, 1 service)");
}

#[test]
fn typo_field() {
    insta::assert_snapshot!(render("server.yaml", &server("  web:\n    client: home\n    bnd: 0.0.0.0:4000\n    target: 127.0.0.1:3000\n"), None), @r#"
    error: service "web" has no "bind" and no "visitors", so nobody can reach it
      --> server.yaml:8:3
       |
     8 |   web:
       |   ^^^ no bind and no visitors
       = help: add "bind: 7840" to expose it publicly, or "visitors: [NAME]" for visitors using burrow visit

    error: unknown field "bnd" in service "web"
      --> server.yaml:10:5
       |
    10 |     bnd: 0.0.0.0:4000
       |     ^^^ unknown field
       = help: did you mean "bind"? valid fields: client, clients, mode, health_check, bind, target, protocol, max_connections, visitors, allow_from, deny_from, proxy_protocol, bandwidth

    server.yaml: 2 errors, 0 warnings
    "#);
}

#[test]
fn unknown_client() {
    insta::assert_snapshot!(render("server.yaml", &server("  web:\n    client: hom\n    bind: 0.0.0.0:4000\n    target: 127.0.0.1:3000\n"), None), @r#"
    error: unknown client "hom" referenced by service "web"
      --> server.yaml:9:13
       |
     9 |     client: hom
       |             ^^^ no client with this name
       = help: did you mean "home"? defined clients: home, office

    server.yaml: 1 error, 0 warnings
    "#);
}

#[test]
fn wrong_types() {
    insta::assert_snapshot!(render("server.yaml", &server(
        "  web:\n    client: home\n    bind: [1, 2]\n    target: 127.0.0.1:3000\n    max_connections: \"10\"\nheartbeat:\n  interval_secs: 0\ntcp:\n  nodelay: yes\n"
    ), None), @r#"
    error: "bind" in service "web" must be a whole number or a string, found a list
      --> server.yaml:10:11
       |
    10 |     bind: [1, 2]
       |           ^^^^^^ expected a whole number or a string
       = help: use a single value, not a list

    error: "max_connections" in service "web" must be a whole number, found a string
      --> server.yaml:12:22
       |
    12 |     max_connections: "10"
       |                      ^^^^ expected a whole number
       = help: remove the quotes: 10

    error: "interval_secs" in "heartbeat" must be between 1 and 3600, found 0
      --> server.yaml:14:18
       |
    14 |   interval_secs: 0
       |                  ^ must be between 1 and 3600
       = help: use a value between 1 and 3600

    error: "nodelay" in "tcp" must be true or false, found a string
      --> server.yaml:16:12
       |
    16 |   nodelay: yes
       |            ^^^ expected true or false
       = help: use true or false

    server.yaml: 4 errors, 0 warnings
    "#);
}

#[test]
fn missing_fields() {
    insta::assert_snapshot!(render("server.yaml", "clients:\n  home:\n    secret: x\nservices:\n  web:\n    client: home\n", None), @r#"
    error: the server needs "listen" (TCP), "websocket", or both
      --> server.yaml
       = help: add "listen: 0.0.0.0:7835" for TCP clients, or a websocket section for clients behind HTTP-only networks (see https://github.com/charlesng35/burrow/blob/main/docs/websocket.md)

    error: secret for client "home" is not valid base64 or hex (1 character)
      --> server.yaml:3:13
       = help: use a 32-byte value encoded as base64 or hex; generate one with: burrow gensecret

    error: missing field "target" in service "web"
      --> server.yaml:5:3
       |
     5 |   web:
       |   ^^^ "target" is required here
       = help: add "target: ..." (host:port as seen from the client, or a port alone for 127.0.0.1 on the client)

    server.yaml: 3 errors, 0 warnings
    "#);
}

#[test]
fn bad_protocol() {
    insta::assert_snapshot!(render("server.yaml", &server(
        "  dns:\n    client: home\n    protocol: tpc\n    bind: 5353\n    target: 192.168.1.1:53\n"
    ), None), @r#"
    error: unknown protocol "tpc" in service "dns"
      --> server.yaml:10:15
       |
    10 |     protocol: tpc
       |               ^^^ not a valid protocol
       = help: did you mean "tcp"? valid values: tcp, udp

    server.yaml: 1 error, 0 warnings
    "#);
}

#[test]
fn duplicate_binds() {
    insta::assert_snapshot!(render("server.yaml", &server(
        "  a:\n    client: home\n    bind: 0.0.0.0:4000\n    target: 127.0.0.1:1\n  b:\n    client: office\n    bind: 127.0.0.1:4000\n    target: 127.0.0.1:2\n  c:\n    client: home\n    bind: 7835\n    target: 127.0.0.1:3\n  d:\n    client: home\n    protocol: udp\n    bind: 4000\n    target: 127.0.0.1:4\n"
    ), None), @r#"
    error: service "b" binds tcp 127.0.0.1:4000, already used by service "a"
      --> server.yaml:14:11
       |
    14 |     bind: 127.0.0.1:4000
       |           ^^^^^^^^^^^^^^ address already used
       = help: pick a different port for one of the services

    error: service "c" binds 0.0.0.0:7835, which is the control port
      --> server.yaml:18:11
       |
    18 |     bind: 7835
       |           ^^^^ same as "listen"
       = help: pick a port other than 7835

    server.yaml: 2 errors, 0 warnings
    "#);
}

#[test]
fn sensitive_port() {
    insta::assert_snapshot!(render("server.yaml", &server("  ssh:\n    client: home\n    bind: 0.0.0.0:22\n    target: 192.168.1.50:22\n"), None), @r#"
    warning: service "ssh" exposes port 22 (SSH) on all interfaces
      --> server.yaml:10:11
       |
    10 |     bind: 0.0.0.0:22
       |           ^^^^^^^^^^ reachable from the internet
       = help: bind to 127.0.0.1:22 unless it must be public

    server.yaml: valid server config (2 clients, 1 service), 1 warning
    "#);
}

#[test]
fn weak_and_duplicate_secrets() {
    insta::assert_snapshot!(render("server.yaml", &format!(
        "listen: 0.0.0.0:7835\nclients:\n  a:\n    secret: c2hvcnRzZWNyZXQ=\n  b:\n    secret: \"not base64!\"\n  c:\n    secret: {SECRET}\n  d:\n    secret: [{SECRET}]\n  e:\n    secret: AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\nservices: {{}}\n"
    ), None), @r#"
    error: secret for client "a" is too short (11 bytes, need 32)
      --> server.yaml:4:13
       = help: generate one with: burrow gensecret

    error: secret for client "b" is not valid base64 or hex (11 characters)
      --> server.yaml:6:13
       = help: use a 32-byte value encoded as base64 or hex; generate one with: burrow gensecret

    error: client "d" uses the same secret as client "c"
      --> server.yaml:10:14
       = help: every client needs its own secret; generate one with: burrow gensecret

    error: secret for client "e" does not look random (too many repeated bytes)
      --> server.yaml:12:13
       = help: generate one with: burrow gensecret

    warning: no services are defined, so nothing is exposed
      --> server.yaml:13:1
       |
    13 | services: {}
       | ^^^^^^^^
       = help: add a service, for example:
             services:
               web: { client: NAME, bind: 0.0.0.0:7840, target: 127.0.0.1:80 }

    server.yaml: 4 errors, 1 warning
    "#);
}

#[cfg(unix)]
#[test]
fn unset_env() {
    insta::assert_snapshot!(render("server.yaml", "listen: 0.0.0.0:7835\nclients:\n  home:\n    secret: ${MISSING_SECRET}\nservices:\n  web:\n    client: home\n    bind: 8080\n    target: ${MISSING_TARGET}\n", None), @r#"
    error: environment variable "MISSING_SECRET" is not set
      --> server.yaml:4:13
       |
     4 |     secret: ${MISSING_SECRET}
       |             ^^^^^^^^^^^^^^^^^ referenced here
       = help: set it before starting burrow, for example: export MISSING_SECRET="..."; a secret has the same value in server.yaml and in the client's or visitor's file (make a new one with: burrow gensecret)

    error: environment variable "MISSING_TARGET" is not set
      --> server.yaml:9:13
       |
     9 |     target: ${MISSING_TARGET}
       |             ^^^^^^^^^^^^^^^^^ referenced here
       = help: set it before starting burrow, for example: export MISSING_TARGET="..."

    server.yaml: 2 errors, 0 warnings
    "#);
}

#[test]
fn bad_addresses() {
    insta::assert_snapshot!(render("server.yaml", &server(
        "  a:\n    client: home\n    bind: 0.0.0.0:99999\n    target: localhost\n  b:\n    client: home\n    bind: nowhere:80\n    target: \"::1:22\"\n"
    ), None), @r#"
    error: invalid bind address for service "a": "99999" is not a valid port (1-65535)
      --> server.yaml:10:11
       |
    10 |     bind: 0.0.0.0:99999
       |           ^^^^^^^^^^^^^ expected a port, a range or ip:port
       = help: use a port like 7840, a range like 30000-30010, or ip:port like 127.0.0.1:7840

    error: invalid target for service "a": "localhost" is missing a port
      --> server.yaml:11:13
       |
    11 |     target: localhost
       |             ^^^^^^^^^ expected host:port or a port
       = help: use host:port as seen from the client, for example 127.0.0.1:3000, a port alone like 3000, a range like 30000-30010, or unix:/path/to.sock

    error: invalid bind address for service "b": "nowhere:80" is not a valid bind address
      --> server.yaml:14:11
       |
    14 |     bind: nowhere:80
       |           ^^^^^^^^^^ expected a port, a range or ip:port
       = help: use a port like 7840, a range like 30000-30010, or ip:port like 127.0.0.1:7840

    error: invalid target for service "b": "::1:22" looks like an IPv6 address; write it in brackets, like [::1]:22
      --> server.yaml:15:13
       |
    15 |     target: "::1:22"
       |             ^^^^^^^^ expected host:port or a port
       = help: use host:port as seen from the client, for example 127.0.0.1:3000, a port alone like 3000, a range like 30000-30010, or unix:/path/to.sock

    server.yaml: 4 errors, 0 warnings
    "#);
}

#[test]
fn syntax_error_yaml() {
    insta::assert_snapshot!(render("server.yaml", "listen: 0.0.0.0:7835\nclients:\n  home:\n   secret: [unclosed\nservices: {}\n", None), @r#"
    error: invalid syntax: illegal placement of ':' indicator
      --> server.yaml:5:9
       |
     5 | services: {}
       |         ^ here
       = help: check the indentation and the "key: value" structure around this line

    server.yaml: 1 error, 0 warnings
    "#);
}

#[test]
fn empty_file() {
    insta::assert_snapshot!(render("server.yaml", "", None), @r#"
    error: the config file is empty
      --> server.yaml
       = help: start with "listen: 0.0.0.0:7835" (server) or "server: host:7835" (client), or run: burrow init

    server.yaml: 1 error, 0 warnings
    "#);
}

#[test]
fn insecure_and_empty() {
    insta::assert_snapshot!(render("server.yaml", &format!(
        "listen: 0.0.0.0:7835\ninsecure_plaintext: true\nclients:\n  home:\n    secret: {SECRET}\nservices: {{}}\n"
    ), None), @"
    warning: insecure_plaintext is enabled: forwarded data is not encrypted
      --> server.yaml:2:21
       |
     2 | insecure_plaintext: true
       |                     ^^^^ traffic can be read and modified on the network
       = help: remove this line unless the network between client and server is trusted

    warning: no services are defined, so nothing is exposed
      --> server.yaml:6:1
       |
     6 | services: {}
       | ^^^^^^^^
       = help: add a service, for example:
             services:
               web: { client: NAME, bind: 0.0.0.0:7840, target: 127.0.0.1:80 }

    server.yaml: valid server config (1 client, 0 services), 2 warnings
    ");
}

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
       = help: did you mean "bind"? valid fields: client, clients, mode, health_check, bind, target, protocol, max_connections, visitors, allow_from, deny_from, proxy_protocol, bandwidth

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

fn visitors_server(visitors: &str, services: &str) -> String {
    format!(
        "listen: 7835\nclients:\n  home:\n    secret: {SECRET}\nvisitors:\n{visitors}services:\n{services}"
    )
}

#[test]
fn private_services_and_visitors() {
    use burrow::config::Loaded;
    let src = visitors_server(
        "  laptop:\n    secret: ${HOME_SECRET}\n    reach:\n      home: any\n",
        "  web: { client: home, bind: 7840, target: 3000, visitors: [laptop] }\n  db: { client: home, target: 5432, visitors: [laptop] }\n",
    );
    insta::assert_snapshot!(render("server.yaml", &src, None), @r#"
    warning: visitor "laptop" may reach any target that client "home" allows, over TCP or UDP
      --> server.yaml:9:7
       |
     9 |       home: any
       |       ^^^^ limited only by the client's allow list
       = help: keep "allow" on client "home" narrow, for example allow: [127.0.0.1/32:22], or list only the services this visitor needs

    server.yaml: valid server config (1 client, 1 visitor, 2 services), 1 warning
    "#);
    let o = load_str("server.yaml", &src, Format::Yaml, None, &env);
    let Some(Loaded::Server(s, _)) = o.loaded else {
        panic!("not a server")
    };
    assert_eq!(s.services["db"].bind, None);
    assert_eq!(s.services["db"].visitors, ["laptop"]);
    assert_eq!(s.services["web"].visitors, ["laptop"]);
    assert!(s.visitors["laptop"].reach.contains_key("home"));
}

#[test]
fn visitor_mistakes_in_the_server_config() {
    let src = visitors_server(
        "  laptop:\n    secret: SECRET_A\n    reach:\n      hom: any\n  idle:\n    secret: ${HOME_SECRET}\n",
        "  db: { client: home, target: 5432, visitors: [lapto] }\n  dns: { client: home, protocol: udp, target: 53, visitors: [laptop] }\n  nobody: { client: home, target: 1 }\n",
    )
    .replace("SECRET_A", SECRET);
    insta::assert_snapshot!(render("server.yaml", &src, None), @r#"
    error: visitor "laptop" uses the same secret as client "home"
      --> server.yaml:7:13
       = help: every client and visitor needs its own secret; generate one with: burrow gensecret

    error: unknown client "hom" referenced by "reach" of visitor "laptop"
      --> server.yaml:9:7
       |
     9 |       hom: any
       |       ^^^ no client with this name
       = help: did you mean "home"? defined clients: home

    warning: visitor "idle" cannot reach anything
      --> server.yaml:10:3
       |
    10 |   idle:
       |   ^^^^
       = help: add it to a service, for example visitors: [idle], or give it reach: { CLIENT: any }

    error: unknown visitor "lapto" referenced by service "db"
      --> server.yaml:13:48
       |
    13 |   db: { client: home, target: 5432, visitors: [lapto] }
       |                                                ^^^^^ no visitor with this name
       = help: did you mean "laptop"? defined visitors: laptop, idle

    error: service "nobody" has no "bind" and no "visitors", so nobody can reach it
      --> server.yaml:15:3
       |
    15 |   nobody: { client: home, target: 1 }
       |   ^^^^^^ no bind and no visitors
       = help: add "bind: 7840" to expose it publicly, or "visitors: [NAME]" for visitors using burrow visit

    server.yaml: 4 errors, 1 warning
    "#);
}

#[test]
fn a_udp_service_with_neither_bind_nor_visitors_is_told_to_add_one() {
    let src = visitors_server(
        "  laptop:\n    secret: ${HOME_SECRET}\n",
        "  web: { client: home, bind: 7840, target: 80, visitors: [laptop] }\n  dns: { client: home, protocol: udp, target: 53 }\n",
    );
    insta::assert_snapshot!(render("server.yaml", &src, None), @r#"
    error: service "dns" has no "bind" and no "visitors", so nobody can reach it
      --> server.yaml:10:3
       |
    10 |   dns: { client: home, protocol: udp, target: 53 }
       |   ^^^ no bind and no visitors
       = help: add "bind: 7840" to expose it publicly, or "visitors: [NAME]" for visitors using burrow visit

    server.yaml: 1 error, 0 warnings
    "#);
}

#[test]
fn a_private_udp_service_with_visitors_is_valid() {
    let src = visitors_server(
        "  laptop:\n    secret: ${HOME_SECRET}\n",
        "  dns: { client: home, protocol: udp, target: 53, visitors: [laptop] }\n  game: { client: home, protocol: udp, target: 27015-27016, visitors: [laptop] }\n",
    );
    insta::assert_snapshot!(render("server.yaml", &src, None), @"server.yaml: valid server config (1 client, 1 visitor, 2 services)");
}

#[test]
fn udp_forwards_in_a_visitor_config() {
    insta::assert_snapshot!(render("visitor.yaml", &visitor(
        "  dns: { service: dns, protocol: udp, local: 5353 }\n  db: { service: db, local: 5353 }\n  dup: { service: x, protocol: udp, local: 5353 }\n  proxy: { client: home, socks: true, protocol: udp, local: 1080 }\n  sock: { client: home, target: \"unix:/run/a.sock\", protocol: udp, local: 9 }\n"
    ), None), @r#"
    error: forward "dup" listens on 127.0.0.1:5353, already used by forward "dns"
      --> visitor.yaml:7:44
       |
     7 |   dup: { service: x, protocol: udp, local: 5353 }
       |                                            ^^^^ address already used
       = help: pick a different local port for one of the forwards

    error: forward "proxy" is a SOCKS proxy, which listens on tcp
      --> visitor.yaml:8:49
       |
     8 |   proxy: { client: home, socks: true, protocol: udp, local: 1080 }
       |                                                 ^^^ udp with socks: true
       = help: remove "protocol: udp", or use a forward with service, or client and target, for UDP

    error: forward "sock" is udp, but its target is a Unix socket, which carries tcp only
      --> visitor.yaml:9:63
       |
     9 |   sock: { client: home, target: "unix:/run/a.sock", protocol: udp, local: 9 }
       |                                                               ^^^ udp to a Unix socket
       = help: remove "protocol: udp", or use a host:port target

    visitor.yaml: 3 errors, 0 warnings
    "#);
    insta::assert_snapshot!(render("visitor.yaml", &format!(
        "{}udp:\n  session_timeout_secs: 120\n  max_peers: 100\n",
        visitor("  dns: { service: dns, protocol: udp, local: 5353 }\n  lan: { client: home, target: \"10.0.0.1:27015-27016\", protocol: udp, local: 27015-27016 }\n")
    ), None), @r#"visitor.yaml: valid visitor config for "laptop" (2 forwards)"#);
    insta::assert_snapshot!(render("visitor.yaml", &visitor(
        "  dns: { service: dns, protocol: sctp, local: 5353 }\n"
    ), None), @r#"
    error: unknown protocol "sctp" in forward "dns"
      --> visitor.yaml:5:34
       |
     5 |   dns: { service: dns, protocol: sctp, local: 5353 }
       |                                  ^^^^ not a valid protocol
       = help: valid values: tcp, udp

    visitor.yaml: 1 error, 0 warnings
    "#);
}

fn visitor(forwards: &str) -> String {
    format!("server: tunnel.example.com\nname: laptop\nsecret: {SECRET}\nforwards:\n{forwards}")
}

#[test]
fn valid_visitor() {
    use burrow::config::Loaded;
    use burrow::wire::protocol::VisitTarget;
    let src = visitor(
        "  db: { service: db, local: 5432 }\n  ssh: { client: home, target: 22, local: \"2222\" }\n",
    );
    insta::assert_snapshot!(render("visitor.yaml", &src, None), @r#"visitor.yaml: valid visitor config for "laptop" (2 forwards)"#);
    let o = load_str("visitor.yaml", &src, Format::Yaml, None, &env);
    assert_eq!(o.role, Some(Role::Visitor));
    let Some(Loaded::Visitor(s, _)) = o.loaded else {
        panic!("not a visitor")
    };
    assert_eq!(s.server, "tunnel.example.com:7835");
    assert_eq!(s.forwards["db"].local, "127.0.0.1:5432".parse().unwrap());
    assert_eq!(s.forwards["db"].to, VisitTarget::service("db"));
    assert_eq!(
        s.forwards["ssh"].to,
        VisitTarget::Client {
            client: "home".into(),
            target: "127.0.0.1:22".into()
        }
    );
    assert_eq!(s.forwards["ssh"].local, "127.0.0.1:2222".parse().unwrap());
}

#[test]
fn visitor_mistakes() {
    insta::assert_snapshot!(render("visitor.yaml", &visitor(
        "  a: { service: db, client: home, local: 1 }\n  b: { client: home, local: 2 }\n  c: { target: 22, local: 3 }\n  d: { local: 4 }\n  e: { service: db, local: 0.0.0.0:5432 }\n  f: { service: web, local: 5432 }\n  g: { service: x, local: nowhere }\n  h: { client: home, target: \"::1:22\", local: 8 }\n"
    ), None), @r#"
    error: forward "a" has both "service" and "client" or "target"
      --> visitor.yaml:5:3
       |
     5 |   a: { service: db, client: home, local: 1 }
       |   ^ use one or the other
       = help: use "service" for a private service, or "client" and "target" to reach a client directly

    error: forward "b" has "client" but no "target"
      --> visitor.yaml:6:3
       |
     6 |   b: { client: home, local: 2 }
       |   ^ missing target
       = help: add the host:port to reach as seen from the client, for example target: 127.0.0.1:22

    error: forward "c" has "target" but no "client"
      --> visitor.yaml:7:3
       |
     7 |   c: { target: 22, local: 3 }
       |   ^ missing client
       = help: add the client to reach, for example client: home

    error: forward "d" needs "service", or "client" and "target"
      --> visitor.yaml:8:3
       |
     8 |   d: { local: 4 }
       |   ^ nothing to reach
       = help: use "service" for a private service, or "client" and "target" to reach a client directly

    warning: forward "e" listens on all interfaces
      --> visitor.yaml:9:28
       |
     9 |   e: { service: db, local: 0.0.0.0:5432 }
       |                            ^^^^^^^^^^^^ reachable from your network
       = help: use "local: 5432" (127.0.0.1 only) unless other machines should use it: they would get this visitor's access

    error: forward "f" listens on 127.0.0.1:5432, already used by forward "e"
      --> visitor.yaml:10:29
       |
    10 |   f: { service: web, local: 5432 }
       |                             ^^^^ address already used
       = help: pick a different local port for one of the forwards

    error: invalid local address for forward "g": "nowhere" is not a valid local address
      --> visitor.yaml:11:27
       |
    11 |   g: { service: x, local: nowhere }
       |                           ^^^^^^^ expected a port, a range or ip:port
       = help: use a port like 5432 (on 127.0.0.1), a range like 40000-40010, or ip:port like 127.0.0.1:5432

    error: invalid target for forward "h": "::1:22" looks like an IPv6 address; write it in brackets, like [::1]:22
      --> visitor.yaml:12:30
       |
    12 |   h: { client: home, target: "::1:22", local: 8 }
       |                              ^^^^^^^^ expected host:port or a port
       = help: use host:port as seen from the client, for example 127.0.0.1:22, or a port alone like 22

    visitor.yaml: 7 errors, 1 warning
    "#);
    insta::assert_snapshot!(render("visitor.yaml", &visitor("  {}\n"), None), @r#"visitor.yaml: valid visitor config for "laptop" (0 forwards)"#);
    insta::assert_snapshot!(render("visitor.yaml", &format!("server: a\nname: laptop\nsecret: {SECRET}\n"), Some(Role::Visitor)), @r#"
    warning: no forwards are defined
      --> visitor.yaml
       = help: add one, for example:
             forwards:
               db: { service: db, local: 5432 }
           or name the target when you run it: burrow visit --service NAME --local PORT

    visitor.yaml: valid visitor config for "laptop" (0 forwards), 1 warning
    "#);
}

#[test]
fn reach_mistakes_are_explained() {
    let src = |reach: &str| {
        visitors_server(
            &format!("  laptop:\n    secret: ${{HOME_SECRET}}\n    reach: {reach}\n"),
            "  db: { client: home, target: 5432, bind: , visitors: [] }\n",
        )
    };
    insta::assert_snapshot!(render("server.yaml", &src("{ home: all }"), None), @r#"
    error: invalid reach "all" for client "home"
      --> server.yaml:8:20
       |
     8 |     reach: { home: all }
       |                    ^^^ expected any or IP ranges
       = help: write home: any for any target its allow list permits, or narrow it with ranges, for example home: [192.168.1.0/24]

    error: service "db" has no "bind" and no "visitors", so nobody can reach it
      --> server.yaml:10:3
       |
    10 |   db: { client: home, target: 5432, bind: , visitors: [] }
       |   ^^ no bind and no visitors
       = help: add "bind: 7840" to expose it publicly, or "visitors: [NAME]" for visitors using burrow visit

    server.yaml: 2 errors, 0 warnings
    "#);
    insta::assert_snapshot!(render("server.yaml", &src("[home]"), None), @r#"
    error: "reach" in visitor "laptop" must be a mapping, found a list
      --> server.yaml:8:12
       |
     8 |     reach: [home]
       |            ^^^^^^ expected a mapping
       = help: write it as a mapping of client names, for example: reach: { home: any }

    error: service "db" has no "bind" and no "visitors", so nobody can reach it
      --> server.yaml:10:3
       |
    10 |   db: { client: home, target: 5432, bind: , visitors: [] }
       |   ^^ no bind and no visitors
       = help: add "bind: 7840" to expose it publicly, or "visitors: [NAME]" for visitors using burrow visit

    server.yaml: 2 errors, 0 warnings
    "#);
}

#[test]
fn reach_alone_is_not_nothing_exposed() {
    let src = format!(
        "listen: 7835\nclients:\n  home:\n    secret: {SECRET}\nvisitors:\n  laptop:\n    secret: ${{HOME_SECRET}}\n    reach:\n      home: any\n"
    );
    let text = render("server.yaml", &src, None);
    assert!(!text.contains("no services are defined"), "{text}");
}

#[test]
fn misspelled_secret_keys_are_hidden_too() {
    let text = render(
        "server.yaml",
        &visitors_server(
            &format!("  laptop:\n    secrte: {SECRET}\n"),
            "  db: { client: home, target: 5432, visitors: [laptop] }\n",
        )
        .replace(&format!("secret: {SECRET}"), &format!("secret: {SECRET2}")),
        None,
    );
    assert!(text.contains("unknown field \"secrte\""), "{text}");
    let text = render(
        "visitor.yaml",
        &format!("server: a\nname: laptop\nsecrets: {SECRET}\nforwards: {{}}\n"),
        None,
    );
    assert!(text.contains("unknown field \"secrets\""), "{text}");
}

#[test]
fn visitor_files_are_recognised_without_forwards() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("visitor.yaml");
    std::fs::write(
        &path,
        format!("server: a\nname: laptop\nsecret: {SECRET}\n"),
    )
    .unwrap();
    let o = burrow::config::load_file(&path, None, &env).unwrap();
    assert_eq!(o.role, Some(Role::Visitor));
    assert!(!o.report.has_errors());
    let typo = render(
        "my.yaml",
        &format!("server: a\nname: laptop\nsecret: {SECRET}\nforward: {{}}\n"),
        None,
    );
    assert!(typo.contains("did you mean \"forwards\"?"), "{typo}");
}

#[test]
fn source_filters() {
    insta::assert_snapshot!(render("server.yaml", &server("  ssh:\n    client: home\n    bind: 2222\n    target: 22\n    allow_from: [203.0.113.0/24, 2001:db8::/32]\n    deny_from: [203.0.113.66]\n"), None), @"server.yaml: valid server config (2 clients, 1 service)");
    insta::assert_snapshot!(render("server.yaml", &server("  ssh:\n    client: home\n    bind: 2222\n    target: 22\n    allow_from: [office, 10.0.0.1/8]\n    deny_from: [10.0.0.0/8:22]\n"), None), @r#"
    error: invalid allow_from entry for service "ssh": "office" is not an IP address or CIDR
      --> server.yaml:12:18
       |
    12 |     allow_from: [office, 10.0.0.1/8]
       |                  ^^^^^^ expected an IP or CIDR
       = help: use an IP like 203.0.113.7 or a network like 203.0.113.0/24 or 2001:db8::/32

    error: invalid allow_from entry for service "ssh": "10.0.0.1/8" has host bits set
      --> server.yaml:12:26
       |
    12 |     allow_from: [office, 10.0.0.1/8]
       |                          ^^^^^^^^^^ expected an IP or CIDR
       = help: use an IP like 203.0.113.7 or a network like 203.0.113.0/24 or 2001:db8::/32

    error: invalid deny_from entry for service "ssh": "10.0.0.0/8:22" is not an IP address or CIDR
      --> server.yaml:13:17
       |
    13 |     deny_from: [10.0.0.0/8:22]
       |                 ^^^^^^^^^^^^^ expected an IP or CIDR
       = help: use an IP like 203.0.113.7 or a network like 203.0.113.0/24 or 2001:db8::/32

    server.yaml: 3 errors, 0 warnings
    "#);
}

#[test]
fn proxy_protocol_needs_tcp() {
    insta::assert_snapshot!(render("server.yaml", &server("  web:\n    client: home\n    bind: 8443\n    target: 8443\n    proxy_protocol: v2\n  dns:\n    client: home\n    bind: 5353\n    target: 53\n    protocol: udp\n    proxy_protocol: v1\n"), None), @r#"
    error: service "dns" is udp, but proxy_protocol only works for tcp services
      --> server.yaml:18:21
       |
    18 |     proxy_protocol: v1
       |                     ^^
       = help: remove "proxy_protocol", or use a tcp service

    server.yaml: 1 error, 0 warnings
    "#);
    insta::assert_snapshot!(render("server.yaml", &server("  web:\n    client: home\n    bind: 8443\n    target: 8443\n    proxy_protocol: v3\n"), None), @r#"
    error: unknown proxy_protocol "v3" in service "web"
      --> server.yaml:12:21
       |
    12 |     proxy_protocol: v3
       |                     ^^ not a valid proxy_protocol
       = help: did you mean "v1"? valid values: v1, v2

    server.yaml: 1 error, 0 warnings
    "#);
}

#[test]
fn port_ranges() {
    use burrow::config::Loaded;
    let ok = server(
        "  game:\n    client: home\n    bind: 30000-30010\n    target: 192.168.1.50:30000-30010\n    protocol: udp\n  media:\n    client: home\n    bind: 127.0.0.1:40000-40001\n    target: 50000-50001\n",
    );
    insta::assert_snapshot!(render("server.yaml", &ok, None), @"server.yaml: valid server config (2 clients, 2 services)");
    let o = load_str("server.yaml", &ok, Format::Yaml, None, &env);
    let Some(Loaded::Server(s, _)) = o.loaded else {
        panic!("server config loads")
    };
    let game = &s.services["game"];
    assert_eq!((game.bind.unwrap().port(), game.ports), (30000, 11));
    assert_eq!(game.target_at(10), "192.168.1.50:30010");
    assert_eq!(s.services["media"].target_at(1), "127.0.0.1:50001");
    insta::assert_snapshot!(render("server.yaml", &server("  a:\n    client: home\n    bind: 30000-30010\n    target: 30000-30005\n  b:\n    client: home\n    bind: 30005-30020\n    target: 40000-40015\n  c:\n    client: home\n    bind: 7830-7840\n    target: 1000-1010\n  d:\n    client: home\n    bind: 1-2000\n    target: 1-2000\n  e:\n    client: home\n    bind: 20-30\n    target: 20-30\n  f:\n    client: home\n    bind: 9-5\n    target: 1\n"), None), @r#"
    error: service "a" binds 11 ports but targets 6
      --> server.yaml:11:13
       |
    11 |     target: 30000-30005
       |             ^^^^^^^^^^^ not the same number of ports as bind
       = help: give bind and target ranges of the same size, for example bind: 30000-30010 and target: 30000-30010

    error: service "b" binds tcp 0.0.0.0:30005-30020, already used by service "a"
      --> server.yaml:14:11
       |
    14 |     bind: 30005-30020
       |           ^^^^^^^^^^^ address already used
       = help: pick a different port for one of the services

    error: service "c" binds 0.0.0.0:7830-7840, which includes the control port
      --> server.yaml:18:11
       |
    18 |     bind: 7830-7840
       |           ^^^^^^^^^ same as "listen"
       = help: pick a port other than 7835

    error: invalid bind address for service "d": the range 1-2000 has 2000 ports, the limit is 1000
      --> server.yaml:22:11
       |
    22 |     bind: 1-2000
       |           ^^^^^^ expected a port, a range or ip:port
       = help: use a port like 7840, a range like 30000-30010, or ip:port like 127.0.0.1:7840

    error: invalid target for service "d": the range 1-2000 has 2000 ports, the limit is 1000
      --> server.yaml:23:13
       |
    23 |     target: 1-2000
       |             ^^^^^^ expected host:port or a port
       = help: use host:port as seen from the client, for example 127.0.0.1:3000, a port alone like 3000, a range like 30000-30010, or unix:/path/to.sock

    warning: service "e" exposes port 22 (SSH) on all interfaces
      --> server.yaml:26:11
       |
    26 |     bind: 20-30
       |           ^^^^^ reachable from the internet
       = help: bind to 127.0.0.1:20-30 unless it must be public

    error: invalid bind address for service "f": the range 9-5 ends before it starts
      --> server.yaml:30:11
       |
    30 |     bind: 9-5
       |           ^^^ expected a port, a range or ip:port
       = help: use a port like 7840, a range like 30000-30010, or ip:port like 127.0.0.1:7840

    server.yaml: 6 errors, 1 warning
    "#);
}

#[test]
fn visitor_port_ranges() {
    insta::assert_snapshot!(render("visitor.yaml", &visitor(
        "  game: { service: game, local: 40000-40010 }\n  ssh: { client: home, target: 30000-30002, local: 2220-2222 }\n",
    ), None), @r#"visitor.yaml: valid visitor config for "laptop" (2 forwards)"#);
    insta::assert_snapshot!(render("visitor.yaml", &visitor(
        "  game: { service: game, local: 40000-40010 }\n  more: { service: more, local: 40010 }\n  ssh: { client: home, target: 30000-30002, local: 2222 }\n",
    ), None), @r#"
    error: forward "more" listens on 127.0.0.1:40010, already used by forward "game"
      --> visitor.yaml:6:33
       |
     6 |   more: { service: more, local: 40010 }
       |                                 ^^^^^ address already used
       = help: pick a different local port for one of the forwards

    error: forward "ssh" listens on 1 port but targets 3
      --> visitor.yaml:7:32
       |
     7 |   ssh: { client: home, target: 30000-30002, local: 2222 }
       |                                ^^^^^^^^^^^ not the same number of ports as local
       = help: give local and target ranges of the same size, for example local: 40000-40010 and target: 30000-30010

    visitor.yaml: 2 errors, 0 warnings
    "#);
}

#[test]
fn unix_socket_targets() {
    insta::assert_snapshot!(render("server.yaml", &server("  db:\n    client: home\n    bind: 5432\n    target: unix:/var/run/postgresql/.s.PGSQL.5432\n"), None), @r#"
    warning: service "db" exposes port 5432 (PostgreSQL) on all interfaces
      --> server.yaml:10:11
       |
    10 |     bind: 5432
       |           ^^^^ reachable from the internet
       = help: bind to 127.0.0.1:5432 unless it must be public

    server.yaml: valid server config (2 clients, 1 service), 1 warning
    "#);
    insta::assert_snapshot!(render("server.yaml", &server("  a:\n    client: home\n    bind: 5432\n    target: unix:run/x.sock\n  b:\n    client: home\n    bind: 5353\n    protocol: udp\n    target: unix:/run/dns.sock\n  c:\n    client: home\n    bind: 5433\n    target: unix:/run/../etc/x.sock\n"), None), @r#"
    warning: service "a" exposes port 5432 (PostgreSQL) on all interfaces
      --> server.yaml:10:11
       |
    10 |     bind: 5432
       |           ^^^^ reachable from the internet
       = help: bind to 127.0.0.1:5432 unless it must be public

    error: invalid target for service "a": socket path "run/x.sock" must be absolute, starting with "/"
      --> server.yaml:11:13
       |
    11 |     target: unix:run/x.sock
       |             ^^^^^^^^^^^^^^^ expected unix:/absolute/path
       = help: use the socket's full path as seen from the client, for example unix:/var/run/postgresql/.s.PGSQL.5432

    error: service "b" is udp, but its target is a Unix socket
      --> server.yaml:16:13
       |
    16 |     target: unix:/run/dns.sock
       |             ^^^^^^^^^^^^^^^^^^
       = help: Unix socket targets work for tcp services only

    error: invalid target for service "c": socket path "/run/../etc/x.sock" may not contain ".."
      --> server.yaml:20:13
       |
    20 |     target: unix:/run/../etc/x.sock
       |             ^^^^^^^^^^^^^^^^^^^^^^^ expected unix:/absolute/path
       = help: use the socket's full path as seen from the client, for example unix:/var/run/postgresql/.s.PGSQL.5432

    server.yaml: 3 errors, 1 warning
    "#);
    let client = |allow: &str| {
        format!("server: tunnel.example.com\nname: home\nsecret: {SECRET}\nallow: {allow}\n")
    };
    insta::assert_snapshot!(render("client.yaml", &client("[127.0.0.0/8, \"unix:/var/run/docker.sock\", \"unix:/run/postgresql/*\"]"), None), @r#"client.yaml: valid client config for "home""#);
    insta::assert_snapshot!(render("client.yaml", &client("[\"unix:relative.sock\", \"unix:/*\"]"), None), @r#"
    error: invalid allow entry: socket path "relative.sock" must be absolute, starting with "/"
      --> client.yaml:4:9
       |
     4 | allow: ["unix:relative.sock", "unix:/*"]
       |         ^^^^^^^^^^^^^^^^^^^^ not a valid socket rule
       = help: use unix:/absolute/path.sock for one socket, or unix:/folder/* for the sockets in a folder

    error: invalid allow entry: allowing every socket on the machine (unix:/*) is too broad; name a folder
      --> client.yaml:4:31
       |
     4 | allow: ["unix:relative.sock", "unix:/*"]
       |                               ^^^^^^^^^ not a valid socket rule
       = help: use unix:/absolute/path.sock for one socket, or unix:/folder/* for the sockets in a folder

    client.yaml: 2 errors, 0 warnings
    "#);
}

#[test]
fn bandwidth_limits() {
    insta::assert_snapshot!(render("server.yaml", &server("  a:\n    client: home\n    bind: 4000\n    target: 4000\n    bandwidth: 40mbit\n  b:\n    client: home\n    bind: 4001\n    target: 4001\n    bandwidth: fast\n  c:\n    client: home\n    bind: 4002\n    target: 4002\n    bandwidth: 5 apples\n"), None), @r#"
    error: invalid bandwidth for service "b": "fast" does not start with a positive number
      --> server.yaml:17:16
       |
    17 |     bandwidth: fast
       |                ^^^^ expected a rate like 40mbit
       = help: use a number with kbit, mbit or gbit (bits per second), or kb, mb or gb (bytes per second), for example 40mbit

    error: invalid bandwidth for service "c": "apples" is not a rate unit
      --> server.yaml:22:16
       |
    22 |     bandwidth: 5 apples
       |                ^^^^^^^^ expected a rate like 40mbit
       = help: use a number with kbit, mbit or gbit (bits per second), or kb, mb or gb (bytes per second), for example 40mbit

    server.yaml: 2 errors, 0 warnings
    "#);
}

#[test]
fn several_clients() {
    use burrow::config::Loaded;
    use burrow_config::Mode;
    let ok = server(
        "  web:\n    clients: [home, office]\n    mode: balance\n    health_check: true\n    bind: 4000\n    target: 4000\n  db:\n    clients: [home, office]\n    health_check: { interval_secs: 5, failures: 3 }\n    bind: 4001\n    target: 4001\n",
    );
    insta::assert_snapshot!(render("server.yaml", &ok, None), @"server.yaml: valid server config (2 clients, 2 services)");
    let o = load_str("server.yaml", &ok, Format::Yaml, None, &env);
    let Some(Loaded::Server(s, _)) = o.loaded else {
        panic!("server config loads")
    };
    let web = &s.services["web"];
    assert_eq!(web.clients, ["home", "office"]);
    assert_eq!(web.mode, Mode::Balance);
    let h = web.health.unwrap();
    assert_eq!(
        (h.interval.as_secs(), h.timeout.as_secs(), h.failures),
        (10, 3, 2)
    );
    let h = s.services["db"].health.unwrap();
    assert_eq!((h.interval.as_secs(), h.failures), (5, 3));
    assert_eq!(s.services["db"].mode, Mode::Failover);
    insta::assert_snapshot!(render("server.yaml", &server("  a:\n    client: home\n    clients: [office]\n    bind: 4000\n    target: 4000\n  b:\n    bind: 4001\n    target: 4001\n  c:\n    clients: [home, home, ofice]\n    bind: 4002\n    target: 4002\n  d:\n    clients: [home, office]\n    mode: balance\n    health_check: true\n    protocol: udp\n    bind: 4003\n    target: 4003\n  e:\n    client: home\n    mode: random\n    health_check: often\n    bind: 4004\n    target: 4004\n"), None), @r#"
    error: service "a" has both "client" and "clients"
      --> server.yaml:10:5
       |
    10 |     clients: [office]
       |     ^^^^^^^ use one or the other
       = help: keep "client: NAME" for one client, or list them all under "clients"

    error: service "b" has no "client"
      --> server.yaml:13:3
       |
    13 |   b:
       |   ^ which client reaches the target?
       = help: add "client: NAME" with a client from "clients:", or "clients: [A, B]" for several

    error: service "c" lists client "home" twice
      --> server.yaml:17:21
       |
    17 |     clients: [home, home, ofice]
       |                     ^^^^
       = help: list each client once

    error: unknown client "ofice" referenced by service "c"
      --> server.yaml:17:27
       |
    17 |     clients: [home, home, ofice]
       |                           ^^^^^ no client with this name
       = help: did you mean "office"? defined clients: home, office

    error: service "d" is udp, but mode balance only works for tcp services
      --> server.yaml:22:11
       |
    22 |     mode: balance
       |           ^^^^^^^
       = help: remove "mode", or use a tcp service

    error: service "d" is udp, but health_check only works for tcp services
      --> server.yaml:23:19
       |
    23 |     health_check: true
       |                   ^^^^
       = help: remove "health_check", or use a tcp service

    error: unknown mode "random" in service "e"
      --> server.yaml:29:11
       |
    29 |     mode: random
       |           ^^^^^^ not a valid mode
       = help: valid values: failover, balance

    error: "health_check" in service "e" must be true or false or a mapping, found a string
      --> server.yaml:30:19
       |
    30 |     health_check: often
       |                   ^^^^^ expected true or false or a mapping
       = help: write it as an indented block of "key: value" lines

    server.yaml: 8 errors, 0 warnings
    "#);
}

#[test]
fn metrics_endpoint() {
    let with = |metrics: &str| {
        format!(
            "{}metrics:\n  listen: {metrics}\n",
            server("  web:\n    client: home\n    bind: 4000\n    target: 4000\n")
        )
    };
    insta::assert_snapshot!(render("server.yaml", &with("9100"), None), @"server.yaml: valid server config (2 clients, 1 service)");
    insta::assert_snapshot!(render("server.yaml", &with("0.0.0.0:9100"), None), @"
    warning: the metrics endpoint listens on 0.0.0.0:9100, not only on this machine
      --> server.yaml:13:11
       |
    13 |   listen: 0.0.0.0:9100
       |           ^^^^^^^^^^^^ reachable from the network
       = help: anyone who reaches it sees client, service and visitor names and traffic; use 127.0.0.1 unless a firewall limits who can connect

    server.yaml: valid server config (2 clients, 1 service), 1 warning
    ");
    insta::assert_snapshot!(render("server.yaml", &with("4000"), None), @r#"
    error: metrics.listen 127.0.0.1:4000 is already used by service "web"
      --> server.yaml:13:11
       |
    13 |   listen: 4000
       |           ^^^^ address already used
       = help: pick another port for the metrics endpoint

    server.yaml: 1 error, 0 warnings
    "#);
    insta::assert_snapshot!(render("server.yaml", &with("0.0.0.0:7835"), None), @"
    error: metrics.listen 0.0.0.0:7835 is already used by the control port (listen)
      --> server.yaml:13:11
       |
    13 |   listen: 0.0.0.0:7835
       |           ^^^^^^^^^^^^ address already used
       = help: pick another port for the metrics endpoint

    server.yaml: 1 error, 0 warnings
    ");
    insta::assert_snapshot!(render("server.yaml", &with("9100-9101"), None), @r#"
    error: invalid metrics listen address "9100-9101"
      --> server.yaml:13:11
       |
    13 |   listen: 9100-9101
       |           ^^^^^^^^^ expected ip:port or a port
       = help: use a port like 9100 (on 127.0.0.1), or ip:port like 127.0.0.1:9100

    server.yaml: 1 error, 0 warnings
    "#);
}

#[test]
fn socks_forwards() {
    insta::assert_snapshot!(render("visitor.yaml", &visitor("  home: { client: home, socks: true, local: 1080 }\n"), None), @r#"visitor.yaml: valid visitor config for "laptop" (1 forward)"#);
    insta::assert_snapshot!(render("visitor.yaml", &visitor(
        "  a: { client: home, socks: true, target: 22, local: 1080 }\n  b: { service: db, socks: true, local: 1081 }\n  c: { client: home, socks: true, local: 1082-1083 }\n",
    ), None), @r#"
    error: forward "a" is a SOCKS proxy, so it needs "client" and no "service" or "target"
      --> visitor.yaml:5:3
       |
     5 |   a: { client: home, socks: true, target: 22, local: 1080 }
       |   ^ socks: true needs only client
       = help: each SOCKS connection names its target; write it as { client: home, socks: true, local: 1080 }

    error: forward "b" is a SOCKS proxy, so it needs "client" and no "service" or "target"
      --> visitor.yaml:6:3
       |
     6 |   b: { service: db, socks: true, local: 1081 }
       |   ^ socks: true needs only client
       = help: each SOCKS connection names its target; write it as { client: home, socks: true, local: 1080 }

    error: forward "c" is a SOCKS proxy, which listens on one port
      --> visitor.yaml:7:42
       |
     7 |   c: { client: home, socks: true, local: 1082-1083 }
       |                                          ^^^^^^^^^ a range
       = help: use a single port, for example local: 1080

    visitor.yaml: 3 errors, 0 warnings
    "#);
}

#[test]
fn a_client_with_more_services_than_fit_in_one_message_is_refused() {
    let many = |n: usize| {
        let services: String = (0..n)
            .map(|i| {
                format!(
                    "  s{i}:\n    client: home\n    bind: {}\n    target: unix:/run/{}{i:04}.sock\n",
                    10000 + i,
                    "x".repeat(85)
                )
            })
            .collect();
        let o = load_str("server.yaml", &server(&services), Format::Yaml, None, &env);
        o.report
            .diagnostics
            .iter()
            .filter(|d| d.is_error())
            .map(|d| (d.code, d.message.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(many(500), vec![]);
    let refused = many(600);
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert_eq!(refused[0].0, "too-many-services");
    assert!(
        refused[0].1.contains("client \"home\" serves 600 services"),
        "{}",
        refused[0].1
    );
}

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
        assert_eq!(errors[0].code, "wrong-role");
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
    assert!(o.report.diagnostics.iter().any(|d| d.code == "wrong-role"));
    let o = load_str(
        "visitor.yaml",
        &client.replace("allow: [127.0.0.0/8]\n", ""),
        Format::Yaml,
        Some(Role::Visitor),
        &env,
    );
    assert!(
        !o.report.diagnostics.iter().any(|d| d.code == "wrong-role"),
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
    assert!(o.report.diagnostics.iter().any(|d| d.code == "wrong-role"));
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
        ["env-unset", "env-unset", "env-unset"],
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
            .any(|d| d.code == "env-unset")
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
        o.report.diagnostics.iter().any(|d| d.code == "no-forwards")
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

fn self_server(self_section: &str, services: &str, visitors: &str) -> String {
    format!(
        "listen: 0.0.0.0:7835\nmetrics:\n  listen: 127.0.0.1:9100\n{self_section}clients:\n  home:\n    secret: {SECRET}\n{visitors}services:\n{services}"
    )
}

#[test]
fn self_services_and_reach_are_valid() {
    let text = render(
        "server.yaml",
        &self_server(
            "self:\n  allow: [127.0.0.1/32, 169.254.169.254/32:80, \"unix:/run/app.sock\"]\n",
            "  db: { client: self, target: 127.0.0.1:5432, visitors: [laptop] }\n  web: { clients: [self, home], bind: 8080, target: 127.0.0.1:3000 }\n  meta: { client: self, bind: 8081, target: 169.254.169.254:80 }\n  app: { client: self, bind: 8082, target: \"unix:/run/app.sock\" }\n  game: { client: self, protocol: udp, bind: 30000-30001, target: 127.0.0.1:40000-40001 }\n",
            &format!(
                "visitors:\n  laptop:\n    secret: {SECRET2}\n    reach: {{ self: [127.0.0.1/32:22] }}\n"
            ),
        ),
        None,
    );
    insta::assert_snapshot!(text, @"server.yaml: valid server config (1 client, 1 visitor, 5 services)");
}

#[test]
fn a_self_service_without_self_allow_reaches_its_own_target() {
    let text = render(
        "server.yaml",
        &self_server(
            "",
            "  web: { client: self, bind: 8080, target: localhost:3000 }\n",
            "",
        ),
        None,
    );
    insta::assert_snapshot!(text, @"server.yaml: valid server config (1 client, 1 service)");
}

#[test]
fn a_wildcard_listen_port_on_another_machine_is_not_burrow() {
    let text = render(
        "server.yaml",
        &self_server(
            "self:\n  allow: [192.0.2.0/24, 127.0.0.1/32]\n",
            "  chain: { client: self, bind: 8092, target: 192.0.2.9:7835 }\n  peer: { client: self, bind: 8093, target: 192.0.2.9:8093 }\n  loop: { client: self, bind: 8094, target: 127.0.0.1:7835 }\n",
            "",
        ),
        None,
    );
    insta::assert_snapshot!(text, @r#"
    error: service "loop" on "self" targets 127.0.0.1:7835, which is burrow's own listen (0.0.0.0:7835)
      --> server.yaml:12:45
       |
    12 |   loop: { client: self, bind: 8094, target: 127.0.0.1:7835 }
       |                                             ^^^^^^^^^^^^^^ a loop back into burrow
       = help: point the service at the application behind it, on another port

    server.yaml: 1 error, 0 warnings
    "#);
}

#[test]
fn udp_self_targets_are_burrows_own_only_on_its_udp_binds() {
    let text = render(
        "server.yaml",
        &self_server(
            "self:\n  allow: [127.0.0.1/32]\n",
            "  wg: { client: self, protocol: udp, bind: 8100, target: 127.0.0.1:7835 }\n  stats: { client: self, protocol: udp, bind: 8101, target: 127.0.0.1:9100 }\n  echo: { client: self, protocol: udp, bind: 8102, target: 127.0.0.1:8102 }\n  quic: { client: self, protocol: udp, bind: 7835, target: 127.0.0.1:7835 }\n  loop: { client: self, bind: 8103, target: 127.0.0.1:7835 }\n",
            "",
        ),
        None,
    );
    insta::assert_snapshot!(text, @r#"
    error: service "echo" on "self" targets 127.0.0.1:8102, which is burrow's own bind of this service (0.0.0.0:8102)
      --> server.yaml:12:60
       |
    12 |   echo: { client: self, protocol: udp, bind: 8102, target: 127.0.0.1:8102 }
       |                                                            ^^^^^^^^^^^^^^ a loop back into burrow
       = help: point the service at the application behind it, on another port

    error: service "quic" on "self" targets 127.0.0.1:7835, which is burrow's own bind of this service (0.0.0.0:7835)
      --> server.yaml:13:60
       |
    13 |   quic: { client: self, protocol: udp, bind: 7835, target: 127.0.0.1:7835 }
       |                                                            ^^^^^^^^^^^^^^ a loop back into burrow
       = help: point the service at the application behind it, on another port

    error: service "loop" on "self" targets 127.0.0.1:7835, which is burrow's own listen (0.0.0.0:7835)
      --> server.yaml:14:45
       |
    14 |   loop: { client: self, bind: 8103, target: 127.0.0.1:7835 }
       |                                             ^^^^^^^^^^^^^^ a loop back into burrow
       = help: point the service at the application behind it, on another port

    server.yaml: 3 errors, 0 warnings
    "#);
}

#[test]
fn self_refusals() {
    let text = render(
        "server.yaml",
        &format!(
            "listen: 0.0.0.0:7835\nmetrics:\n  listen: 127.0.0.1:9100\nself:\n  allow: [127.0.0.0/8:1-9999, 169.254.0.0/16, \"unix:/run/app.sock\"]\nclients:\n  self:\n    secret: {SECRET}\nservices:\n  outside: {{ client: self, bind: 8080, target: 10.0.0.5:3000 }}\n  range: {{ client: self, bind: 8090-8091, target: 127.0.0.1:9999-10000 }}\n  socket: {{ client: self, bind: 8092, target: \"unix:/run/other.sock\" }}\n  loop: {{ client: self, bind: 8093, target: 127.0.0.1:7835 }}\n  metrics: {{ client: self, bind: 8094, target: localhost:9100 }}\n  itself: {{ client: self, bind: 8095, target: 127.0.0.1:8095 }}\n  meta: {{ client: self, bind: 8096, target: 169.254.169.254:80 }}\n"
        ),
        None,
    );
    insta::assert_snapshot!(text, @r#"
    error: a client cannot be named "self"
      --> server.yaml:7:3
       |
     7 |   self:
       |   ^^^^ reserved for this server
       = help: "self" means the server itself, in "client", "clients" and "reach"; give the client another name

    error: service "outside" on "self" targets 10.0.0.5:3000, which self.allow does not permit
      --> server.yaml:10:48
       |
    10 |   outside: { client: self, bind: 8080, target: 10.0.0.5:3000 }
       |                                                ^^^^^^^^^^^^^ not in self.allow
       = help: add "10.0.0.5/32:3000" to self.allow, or a rule that covers it

    error: service "range" on "self" targets 127.0.0.1:10000, which self.allow does not permit
      --> server.yaml:11:51
       |
    11 |   range: { client: self, bind: 8090-8091, target: 127.0.0.1:9999-10000 }
       |                                                   ^^^^^^^^^^^^^^^^^^^^ not in self.allow
       = help: add "127.0.0.1/32:10000" to self.allow, or a rule that covers it

    error: service "socket" on "self" targets unix:/run/other.sock, which self.allow does not permit
      --> server.yaml:12:47
       |
    12 |   socket: { client: self, bind: 8092, target: "unix:/run/other.sock" }
       |                                               ^^^^^^^^^^^^^^^^^^^^^^^ not in self.allow
       = help: add "unix:/run/other.sock" to self.allow, or a rule that covers it

    error: service "loop" on "self" targets 127.0.0.1:7835, which is burrow's own listen (0.0.0.0:7835)
      --> server.yaml:13:45
       |
    13 |   loop: { client: self, bind: 8093, target: 127.0.0.1:7835 }
       |                                             ^^^^^^^^^^^^^^ a loop back into burrow
       = help: point the service at the application behind it, on another port

    error: service "metrics" on "self" targets 127.0.0.1:9100, which is burrow's own metrics.listen (127.0.0.1:9100)
      --> server.yaml:14:48
       |
    14 |   metrics: { client: self, bind: 8094, target: localhost:9100 }
       |                                                ^^^^^^^^^^^^^^ a loop back into burrow
       = help: point the service at the application behind it, on another port

    error: service "itself" on "self" targets 127.0.0.1:8095, which is burrow's own bind of this service (0.0.0.0:8095)
      --> server.yaml:15:47
       |
    15 |   itself: { client: self, bind: 8095, target: 127.0.0.1:8095 }
       |                                               ^^^^^^^^^^^^^^ a loop back into burrow
       = help: point the service at the application behind it, on another port

    error: service "meta" on "self" targets 169.254.169.254:80, a link-local or metadata address
      --> server.yaml:16:45
       |
    16 |   meta: { client: self, bind: 8096, target: 169.254.169.254:80 }
       |                                             ^^^^^^^^^^^^^^^^^^ not named in self.allow
       = help: list the address itself in self.allow, for example 169.254.169.254/32:80; a range that covers it does not count

    server.yaml: 8 errors, 0 warnings
    "#);
}

#[test]
fn reach_to_self_needs_self_allow() {
    let text = render(
        "server.yaml",
        &self_server(
            "",
            "  web: { client: home, bind: 8080, target: 3000 }\n",
            &format!(
                "visitors:\n  laptop:\n    secret: {SECRET2}\n    reach: {{ self: any }}\n  other:\n    secret: 9YTnoioCLHkV1Qo6L3s4s/0BNsE9Go9Ra+7Q7ybDVYo=\n    reach: {{ self: [127.0.0.1/32] }}\n"
            ),
        ),
        None,
    );
    insta::assert_snapshot!(text, @r#"
    error: visitor "laptop" has reach to "self", but the server config has no self.allow
      --> server.yaml:10:14
       |
    10 |     reach: { self: any }
       |              ^^^^ this server would have no limit
       = help: add what this server may reach for visitors, for example:
             self:
               allow: [127.0.0.1/32:22]

    error: visitor "other" has reach to "self", but the server config has no self.allow
      --> server.yaml:13:14
       |
    13 |     reach: { self: [127.0.0.1/32] }
       |              ^^^^ this server would have no limit
       = help: add what this server may reach for visitors, for example:
             self:
               allow: [127.0.0.1/32:22]

    server.yaml: 2 errors, 0 warnings
    "#);
}

#[test]
fn reach_any_to_self_is_warned_like_any_client() {
    let text = render(
        "server.yaml",
        &self_server(
            "self:\n  allow: [127.0.0.1/32]\n",
            "  web: { client: home, bind: 8080, target: 3000 }\n",
            &format!("visitors:\n  laptop:\n    secret: {SECRET2}\n    reach: {{ self: any }}\n"),
        ),
        None,
    );
    insta::assert_snapshot!(text, @r#"
    warning: visitor "laptop" may reach any target that client "self" allows, over TCP or UDP
      --> server.yaml:12:14
       |
    12 |     reach: { self: any }
       |              ^^^^ limited only by the client's allow list
       = help: keep "self.allow" narrow, for example allow: [127.0.0.1/32:22], or list only the services this visitor needs

    server.yaml: valid server config (1 client, 1 visitor, 1 service), 1 warning
    "#);
}

#[test]
fn self_allow_rules_are_checked() {
    let text = render(
        "server.yaml",
        &self_server(
            "self:\n  allow: [127.0.0.1/33, localhost]\n",
            "  web: { client: home, bind: 8080, target: 3000 }\n",
            "",
        ),
        None,
    );
    insta::assert_snapshot!(text, @r#"
    error: invalid self.allow entry: "33" is not a valid prefix length
      --> server.yaml:5:11
       |
     5 |   allow: [127.0.0.1/33, localhost]
       |           ^^^^^^^^^^^^ not a valid CIDR
       = help: use a CIDR like 192.168.1.0/24, optionally with a port (192.168.1.0/24:22) or a range (:8000-8100)

    error: invalid self.allow entry: "localhost" is not an IP address or CIDR
      --> server.yaml:5:25
       |
     5 |   allow: [127.0.0.1/33, localhost]
       |                         ^^^^^^^^^ not a valid CIDR
       = help: self.allow entries are IP ranges, not host names; resolve the host and add its address, for example 192.168.1.10/32

    server.yaml: 2 errors, 0 warnings
    "#);
}

#[test]
fn a_self_allow_covering_a_whole_family_is_warned_about() {
    let text = render(
        "server.yaml",
        &self_server(
            "self:\n  allow: [0.0.0.0/0, \"::/0\", 0.0.0.0/0:443, 10.0.0.0/8]\n",
            "  web: { client: self, bind: 8080, target: 10.0.0.5:3000 }\n",
            "",
        ),
        None,
    );
    insta::assert_snapshot!(text, @r#"
    warning: self.allow 0.0.0.0/0 lets this server dial any IPv4 address for services with client: self and reach: self, the internet included
      --> server.yaml:5:11
       |
     5 |   allow: [0.0.0.0/0, "::/0", 0.0.0.0/0:443, 10.0.0.0/8]
       |           ^^^^^^^^^ no restriction on targets
       = help: list only the networks this server should reach for itself, for example allow: [127.0.0.1/32, 10.0.0.0/24]

    warning: self.allow ::/0 lets this server dial any IPv6 address for services with client: self and reach: self, the internet included
      --> server.yaml:5:22
       |
     5 |   allow: [0.0.0.0/0, "::/0", 0.0.0.0/0:443, 10.0.0.0/8]
       |                      ^^^^^^ no restriction on targets
       = help: list only the networks this server should reach for itself, for example allow: [127.0.0.1/32, 10.0.0.0/24]

    warning: self.allow 0.0.0.0/0:443 lets this server dial any IPv4 address for services with client: self and reach: self, the internet included
      --> server.yaml:5:30
       |
     5 |   allow: [0.0.0.0/0, "::/0", 0.0.0.0/0:443, 10.0.0.0/8]
       |                              ^^^^^^^^^^^^^ no restriction on targets
       = help: list only the networks this server should reach for itself, for example allow: [127.0.0.1/32, 10.0.0.0/24]

    server.yaml: valid server config (1 client, 1 service), 3 warnings
    "#);
}

#[test]
fn cloud_metadata_targets_need_their_own_rule() {
    let text = render(
        "server.yaml",
        &self_server(
            "self:\n  allow: [100.64.0.0/10, \"fd00::/8\", \"fd00:ec2::254\"]\n",
            "  ali: { client: self, bind: 8080, target: 100.100.100.200:80 }\n  aws: { client: self, bind: 8081, target: \"[fd00:ec2::254]:80\" }\n",
            "",
        ),
        None,
    );
    insta::assert_snapshot!(text, @r#"
    error: service "ali" on "self" targets 100.100.100.200:80, a link-local or metadata address
      --> server.yaml:10:44
       |
    10 |   ali: { client: self, bind: 8080, target: 100.100.100.200:80 }
       |                                            ^^^^^^^^^^^^^^^^^^ not named in self.allow
       = help: list the address itself in self.allow, for example 100.100.100.200/32:80; a range that covers it does not count

    server.yaml: 1 error, 0 warnings
    "#);
}

#[test]
fn localhost_targets_need_one_of_their_addresses_in_self_allow() {
    let text = render(
        "server.yaml",
        &format!(
            "listen: \"[::1]:7835\"\nself:\n  allow: [127.0.0.1/32, 10.0.0.0/24]\nclients:\n  home:\n    secret: {SECRET}\nservices:\n  web: {{ client: self, bind: 8080, target: localhost:13000 }}\n  loop: {{ client: self, bind: 8081, target: localhost:7835 }}\n"
        ),
        None,
    );
    insta::assert_snapshot!(text, @r#"
    error: service "loop" on "self" targets [::1]:7835, which is burrow's own listen ([::1]:7835)
      --> server.yaml:9:45
       |
     9 |   loop: { client: self, bind: 8081, target: localhost:7835 }
       |                                             ^^^^^^^^^^^^^^ a loop back into burrow
       = help: point the service at the application behind it, on another port

    server.yaml: 1 error, 0 warnings
    "#);
    let outside = render(
        "server.yaml",
        &self_server(
            "self:\n  allow: [10.0.0.0/24]\n",
            "  web: { client: self, bind: 8080, target: localhost:13000 }\n",
            "",
        ),
        None,
    );
    insta::assert_snapshot!(outside, @r#"
    error: service "web" on "self" targets 127.0.0.1:13000, which self.allow does not permit
      --> server.yaml:10:44
       |
    10 |   web: { client: self, bind: 8080, target: localhost:13000 }
       |                                            ^^^^^^^^^^^^^^^ not in self.allow
       = help: add "127.0.0.1/32:13000" to self.allow, or a rule that covers it

    server.yaml: 1 error, 0 warnings
    "#);
}
