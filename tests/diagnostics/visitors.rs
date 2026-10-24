use burrow::config::{Format, Role, load_str};

use super::{SECRET, SECRET2, env, render};

pub(super) fn visitors_server(visitors: &str, services: &str) -> String {
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

pub(super) fn visitor(forwards: &str) -> String {
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
