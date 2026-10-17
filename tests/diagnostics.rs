use burrow::config::{Format, Role, load_str};
use burrow::diagnostics::Style;

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
    text.push_str(&burrow::validate::summary_line(&o, false));
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
       = help: did you mean "bind"? valid fields: client, bind, target, protocol, max_connections, visitors

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
       = help: add "listen: 0.0.0.0:7835" for TCP clients, or a websocket section for clients behind HTTP-only networks (see README)

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

#[test]
fn unset_env() {
    insta::assert_snapshot!(render("server.yaml", "listen: 0.0.0.0:7835\nclients:\n  home:\n    secret: ${MISSING_SECRET}\nservices:\n  web:\n    client: home\n    bind: 8080\n    target: ${MISSING_TARGET}\n", None), @r#"
    error: environment variable "MISSING_SECRET" is not set
      --> server.yaml:4:13
       |
     4 |     secret: ${MISSING_SECRET}
       |             ^^^^^^^^^^^^^^^^^ referenced here
       = help: set it before starting burrow, for example: export MISSING_SECRET="$(burrow gensecret)" (for a secret) or export MISSING_SECRET=value

    error: environment variable "MISSING_TARGET" is not set
      --> server.yaml:9:13
       |
     9 |     target: ${MISSING_TARGET}
       |             ^^^^^^^^^^^^^^^^^ referenced here
       = help: set it before starting burrow, for example: export MISSING_TARGET="$(burrow gensecret)" (for a secret) or export MISSING_TARGET=value

    server.yaml: 2 errors, 0 warnings
    "#);
}

#[test]
fn bad_addresses() {
    insta::assert_snapshot!(render("server.yaml", &server(
        "  a:\n    client: home\n    bind: 0.0.0.0:99999\n    target: localhost\n  b:\n    client: home\n    bind: nowhere:80\n    target: \"::1:22\"\n"
    ), None), @r#"
    error: invalid bind address for service "a": "0.0.0.0:99999" is not a valid bind address
      --> server.yaml:10:11
       |
    10 |     bind: 0.0.0.0:99999
       |           ^^^^^^^^^^^^^ expected a port or ip:port
       = help: use a port like 7840, or ip:port like 127.0.0.1:7840

    error: invalid target for service "a": "localhost" is missing a port
      --> server.yaml:11:13
       |
    11 |     target: localhost
       |             ^^^^^^^^^ expected host:port or a port
       = help: use host:port as seen from the client, for example 127.0.0.1:3000, or a port alone like 3000

    error: invalid bind address for service "b": "nowhere:80" is not a valid bind address
      --> server.yaml:14:11
       |
    14 |     bind: nowhere:80
       |           ^^^^^^^^^^ expected a port or ip:port
       = help: use a port like 7840, or ip:port like 127.0.0.1:7840

    error: invalid target for service "b": "::1:22" looks like an IPv6 address; write it in brackets, like [::1]:22
      --> server.yaml:15:13
       |
    15 |     target: "::1:22"
       |             ^^^^^^^^ expected host:port or a port
       = help: use host:port as seen from the client, for example 127.0.0.1:3000, or a port alone like 3000

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
    warning: allow_all is enabled: the server can make this client connect anywhere
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
       = help: did you mean "bind"? valid fields: client, bind, target, protocol, max_connections, visitors

    error: unknown protocol "udpp" in service "dns"
      --> server.json:6:83
       |
     6 |     "dns": {"client": "home", "bind": "5353", "target": "1.1.1.1:53", "protocol": "udpp"}
       |                                                                                   ^^^^^^ not a valid protocol
       = help: did you mean "udp"? valid values: tcp, udp

    error: "max_handshakes" in "limits" must be at least 1, found -1
      --> server.json:8:32
       |
     8 |   "limits": {"max_handshakes": -1}
       |                                ^^ must be at least 1
       = help: use 1 or more

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
       = help: did you mean "allow"? valid fields: server, name, secret, allow, allow_all, insecure_plaintext, headers, ca, cipher, proxy, reconnect, connect_timeout_secs, tcp, udp

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
fn role_override_reports_fields_for_that_role() {
    insta::assert_snapshot!(render("server.yaml", "server: a:1\nname: home\n", Some(Role::Server)), @r#"
    error: the server needs "listen" (TCP), "websocket", or both
      --> server.yaml
       = help: add "listen: 0.0.0.0:7835" for TCP clients, or a websocket section for clients behind HTTP-only networks (see README)

    error: unknown field "server" in server config
      --> server.yaml:1:1
       |
     1 | server: a:1
       | ^^^^^^ unknown field
       = help: valid fields: listen, public_address, websocket, clients, visitors, services, insecure_plaintext, heartbeat, limits, pool, tcp, udp

    warning: no clients are defined, so nobody can connect
      --> server.yaml:1:1
       |
     1 | server: a:1
       | ^
       = help: add one, for example:
             clients:
               home: { secret: SECRET }
           with a SECRET from: burrow gensecret

    warning: no services are defined, so nothing is exposed
      --> server.yaml:1:1
       |
     1 | server: a:1
       | ^
       = help: add a service, for example:
             services:
               web: { client: NAME, bind: 0.0.0.0:7840, target: 127.0.0.1:80 }

    error: unknown field "name" in server config
      --> server.yaml:2:1
       |
     2 | name: home
       | ^^^^ unknown field
       = help: valid fields: listen, public_address, websocket, clients, visitors, services, insecure_plaintext, heartbeat, limits, pool, tcp, udp

    server.yaml: 3 errors, 2 warnings
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
       = help: start the path with "/" and leave out spaces, "?" and "#"

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
       = help: add "listen: 0.0.0.0:7835" for TCP clients, or a websocket section for clients behind HTTP-only networks (see README)

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
        "  web: { client: home, bind: 7840, target: 3000, visitors: [laptop] }\n  db: { client: home, target: 5432, visitors: [laptop, laptop] }\n",
    );
    insta::assert_snapshot!(render("server.yaml", &src, None), @r#"
    warning: visitor "laptop" may reach any target that client "home" allows
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
    assert!(s.visitors["laptop"].reach.contains("home"));
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

    error: service "dns" is udp, but visitors can only reach tcp services
      --> server.yaml:14:61
       |
    14 |   dns: { client: home, protocol: udp, target: 53, visitors: [laptop] }
       |                                                             ^^^^^^^^^
       = help: remove "visitors", or use a tcp service

    error: service "nobody" has no "bind" and no "visitors", so nobody can reach it
      --> server.yaml:15:3
       |
    15 |   nobody: { client: home, target: 1 }
       |   ^^^^^^ no bind and no visitors
       = help: add "bind: 7840" to expose it publicly, or "visitors: [NAME]" for visitors using burrow visit

    server.yaml: 5 errors, 1 warning
    "#);
}

fn visitor(forwards: &str) -> String {
    format!("server: tunnel.example.com\nname: laptop\nsecret: {SECRET}\nforwards:\n{forwards}")
}

#[test]
fn valid_visitor() {
    use burrow::config::Loaded;
    use burrow::protocol::VisitTarget;
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
    assert_eq!(s.forwards["db"].to, VisitTarget::Service("db".into()));
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
       |                           ^^^^^^^ expected a port or ip:port
       = help: use a port like 5432 (on 127.0.0.1), or ip:port like 127.0.0.1:5432

    error: invalid target for forward "h": "::1:22" looks like an IPv6 address; write it in brackets, like [::1]:22
      --> visitor.yaml:12:30
       |
    12 |   h: { client: home, target: "::1:22", local: 8 }
       |                              ^^^^^^^^ expected host:port or a port
       = help: use host:port as seen from the client, for example 127.0.0.1:22, or a port alone like 22

    visitor.yaml: 7 errors, 1 warning
    "#);
    insta::assert_snapshot!(render("visitor.yaml", &visitor("  {}\n"), None), @r#"
    warning: no forwards are defined
      --> visitor.yaml:4:1
       |
     4 | forwards:
       | ^^^^^^^^
       = help: add one, for example:
             forwards:
               db: { service: db, local: 5432 }
           or pick the target on the command line with burrow visit --service or --client

    visitor.yaml: valid visitor config for "laptop" (0 forwards), 1 warning
    "#);
    insta::assert_snapshot!(render("visitor.yaml", &format!("server: a\nname: laptop\nsecret: {SECRET}\n"), Some(Role::Visitor)), @r#"
    error: missing field "forwards" in visitor config
      --> visitor.yaml:1:1
       = help: add "forwards: ..." (local ports that lead to private services or client targets, keyed by forward name)

    visitor.yaml: 1 error, 0 warnings
    "#);
}
