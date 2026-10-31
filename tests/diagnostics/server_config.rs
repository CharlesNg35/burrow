use super::{SECRET, render, server};

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
       = help: did you mean "bind"? valid fields: client, clients, mode, health_check, bind, target, protocol, max_connections, visitors, allow_from, deny_from, proxy_protocol, bandwidth, web

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
