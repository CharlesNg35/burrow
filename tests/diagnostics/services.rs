use burrow::config::{Format, load_str};

use super::visitors::visitor;
use super::{SECRET, env, render, server};

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
       = help: remove proxy_protocol:, or set protocol: tcp

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
    let h = web.health.clone().unwrap();
    assert_eq!(
        (h.interval.as_secs(), h.timeout.as_secs(), h.failures),
        (10, 3, 2)
    );
    let h = s.services["db"].health.clone().unwrap();
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
       = help: list each client once, or give each entry its own target

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
       = help: remove mode:, or set protocol: tcp

    error: service "d" is udp, but health_check only works for tcp services
      --> server.yaml:23:19
       |
    23 |     health_check: true
       |                   ^^^^
       = help: remove health_check:, or set protocol: tcp

    error: unknown mode "random" in service "e"
      --> server.yaml:29:11
       |
    29 |     mode: random
       |           ^^^^^^ not a valid mode
       = help: valid values: failover, balance, least_connections, ip_hash

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
fn client_weights() {
    use burrow::config::Loaded;
    use burrow_config::Mode;
    let ok = server(
        "  web:\n    clients: [home, {name: office, weight: 3}]\n    mode: balance\n    bind: 4000\n    target: 4000\n  db:\n    clients: [home, office]\n    mode: least_connections\n    bind: 4001\n    target: 4001\n",
    );
    let o = load_str("server.yaml", &ok, Format::Yaml, None, &env);
    let Some(Loaded::Server(s, _)) = o.loaded else {
        panic!("server config loads")
    };
    assert_eq!(s.services["web"].clients, ["home", "office"]);
    let weights: Vec<u32> = s.services["web"].entries.iter().map(|e| e.weight).collect();
    assert_eq!(weights, [1, 3]);
    assert_eq!(s.services["db"].mode, Mode::LeastConnections);
    insta::assert_snapshot!(render("server.yaml", &server("  a:\n    clients: [home, {name: office, weight: 2}]\n    bind: 4000\n    target: 4000\n  b:\n    clients: [home, {name: office, weight: 0}]\n    mode: balance\n    bind: 4001\n    target: 4001\n  c:\n    clients: [home, {name: office, wieght: 3}]\n    mode: balance\n    bind: 4002\n    target: 4002\n  d:\n    clients: [home, {name: home, weight: 2}]\n    mode: balance\n    bind: 4003\n    target: 4003\n  e:\n    clients: [home, office]\n    mode: least_connections\n    protocol: udp\n    bind: 4004\n    target: 4004\n  f:\n    clients: [home, {name: office, weight: 2}]\n    mode: least_connections\n    bind: 4005\n    target: 4005\n  g:\n    clients: [home, {name: ofice, weight: 2}]\n    mode: balance\n    bind: 4006\n    target: 4006\n"), None), @r#"
    error: service "a" gives client "office" a weight, but weights only apply with mode: balance or ip_hash
      --> server.yaml:9:44
       |
     9 |     clients: [home, {name: office, weight: 2}]
       |                                            ^ only used by mode: balance or ip_hash
       = help: remove the weight, or set mode: balance or ip_hash

    error: "weight" in "services.b.clients[1]" must be between 1 and 1000, found 0
      --> server.yaml:13:44
       |
    13 |     clients: [home, {name: office, weight: 0}]
       |                                            ^ must be between 1 and 1000
       = help: use a value between 1 and 1000

    error: unknown field "wieght" in "services.c.clients[1]"
      --> server.yaml:18:36
       |
    18 |     clients: [home, {name: office, wieght: 3}]
       |                                    ^^^^^^ unknown field
       = help: did you mean "weight"? valid fields: name, target, weight

    error: service "d" lists client "home" twice
      --> server.yaml:23:21
       |
    23 |     clients: [home, {name: home, weight: 2}]
       |                     ^^^^^^^^^^^^^^^^^^^^^^^
       = help: list each client once, or give each entry its own target

    error: service "e" is udp, but mode least_connections only works for tcp services
      --> server.yaml:29:11
       |
    29 |     mode: least_connections
       |           ^^^^^^^^^^^^^^^^^
       = help: remove mode:, or set protocol: tcp

    error: service "f" gives client "office" a weight, but weights only apply with mode: balance or ip_hash
      --> server.yaml:34:44
       |
    34 |     clients: [home, {name: office, weight: 2}]
       |                                            ^ only used by mode: balance or ip_hash
       = help: remove the weight, or set mode: balance or ip_hash

    error: unknown client "ofice" referenced by service "g"
      --> server.yaml:39:28
       |
    39 |     clients: [home, {name: ofice, weight: 2}]
       |                            ^^^^^ no client with this name
       = help: did you mean "office"? defined clients: home, office

    server.yaml: 7 errors, 0 warnings
    "#);
}

#[test]
fn ip_hash_takes_weights_on_tcp_and_is_refused_on_udp() {
    use burrow::config::Loaded;
    use burrow_config::Mode;
    let ok = server(
        "  web:\n    clients: [home, {name: office, weight: 3}]\n    mode: ip_hash\n    bind: 4000\n    target: 4000\n",
    );
    let o = load_str("server.yaml", &ok, Format::Yaml, None, &env);
    let Some(Loaded::Server(s, _)) = o.loaded else {
        panic!("server config loads")
    };
    assert_eq!(s.services["web"].mode, Mode::IpHash);
    let weights: Vec<u32> = s.services["web"].entries.iter().map(|e| e.weight).collect();
    assert_eq!(weights, [1, 3]);
    insta::assert_snapshot!(render("server.yaml", &server("  dns:\n    clients: [home, office]\n    mode: ip_hash\n    protocol: udp\n    bind: 4004\n    target: 4004\n"), None), @r#"
    error: service "dns" is udp, but mode ip_hash only works for tcp services
      --> server.yaml:10:11
       |
    10 |     mode: ip_hash
       |           ^^^^^^^
       = help: remove mode:, or set protocol: tcp

    server.yaml: 1 error, 0 warnings
    "#);
}

#[test]
fn client_targets() {
    use burrow::config::Loaded;
    let ok = server(
        "  web:\n    clients: [home, {name: office, target: 10.0.0.12:3000}, {name: office, target: 10.0.0.13:3000}]\n    bind: 4000\n    target: 3000\n",
    );
    let o = load_str("server.yaml", &ok, Format::Yaml, None, &env);
    let Some(Loaded::Server(s, _)) = o.loaded else {
        panic!("server config loads")
    };
    let targets: Vec<Option<&str>> = s.services["web"]
        .entries
        .iter()
        .map(|e| e.target.as_deref())
        .collect();
    assert_eq!(
        targets,
        [None, Some("10.0.0.12:3000"), Some("10.0.0.13:3000")]
    );
    assert_eq!(s.services["web"].clients, ["home", "office"]);
    insta::assert_snapshot!(render("server.yaml", &server("  a:\n    clients: [home, {name: office, target: \"nope:\"}]\n    bind: 4000\n    target: 4000\n  b:\n    clients: [home, {name: office, target: 30000-30001}]\n    bind: 4001\n    target: 4001\n  c:\n    clients: [{name: office, target: 8080}, {name: office, target: 8080}]\n    bind: 4002\n    target: 4002\n  d:\n    clients: [office, {name: office, target: 4003}]\n    bind: 4003\n    target: 4003\n  e:\n    clients: [home, {name: office, target: unix:/run/app.sock}]\n    protocol: udp\n    bind: 4004\n    target: 4004\n"), None), @r#"
    error: invalid target for client "office" of service "a": "" is not a valid port (1-65535)
      --> server.yaml:9:44
       |
     9 |     clients: [home, {name: office, target: "nope:"}]
       |                                            ^^^^^^^ expected host:port or a port
       = help: use host:port as seen from the client, for example 127.0.0.1:3000, a port alone like 3000, a range like 30000-30010, or unix:/path/to.sock

    error: client "office" of service "b" targets 2 ports, but the service targets 1
      --> server.yaml:13:44
       |
    13 |     clients: [home, {name: office, target: 30000-30001}]
       |                                            ^^^^^^^^^^^ not the same number of ports as the service target
       = help: give it a range of the same size as the service's, for example 30000-30010, or one port

    error: service "c" lists client "office" twice with the same target
      --> server.yaml:17:45
       |
    17 |     clients: [{name: office, target: 8080}, {name: office, target: 8080}]
       |                                             ^^^^^^^^^^^^^^^^^^^^^^^^^^^^
       = help: give each entry of the same client its own target, or list the client once

    error: service "d" lists client "office" twice with the same target
      --> server.yaml:21:23
       |
    21 |     clients: [office, {name: office, target: 4003}]
       |                       ^^^^^^^^^^^^^^^^^^^^^^^^^^^^
       = help: give each entry of the same client its own target, or list the client once

    error: service "e" is udp, but the target of client "office" is a Unix socket
      --> server.yaml:25:44
       |
    25 |     clients: [home, {name: office, target: unix:/run/app.sock}]
       |                                            ^^^^^^^^^^^^^^^^^^
       = help: Unix socket targets work for tcp services only

    server.yaml: 5 errors, 0 warnings
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
