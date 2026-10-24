use std::path::Path;

use burrow::config::env::ProcessEnv;
use burrow::config::{Format, Loaded, Role, load_file};
use burrow::init::client_config;

use super::load;
use super::visitor_config::visitors_server;

#[test]
fn a_client_config_lists_ipv4_before_ipv6_and_unix_sockets_last() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.yaml");
    let secret = burrow::wire::secret::Secret::generate().encode();
    std::fs::write(
        &path,
        format!(
            "listen: 7835\nclients:\n  home:\n    secret: {secret}\nservices:\n  a:\n    client: home\n    bind: 5433\n    target: unix:/run/a.sock\n  b:\n    client: home\n    bind: 5434\n    target: \"[2001:db8::5]:443\"\n  c:\n    client: home\n    bind: 5435\n    target: 10.0.0.9:22\n  d:\n    client: home\n    bind: 5436\n    target: \"[2001:db8::4]:8443\"\n  e:\n    client: home\n    bind: 5437\n    target: 10.0.0.8:80\n"
        ),
    )
    .unwrap();
    let text = client_config(
        Some("home"),
        &path,
        Some("t.example"),
        Format::Yaml,
        &ProcessEnv,
    )
    .unwrap()
    .text;
    let allow: Vec<&str> = text
        .lines()
        .filter_map(|l| l.trim().strip_prefix("- "))
        .map(|l| l.trim_matches(['"', '\'']))
        .collect();
    assert_eq!(
        allow,
        [
            "10.0.0.8/32:80",
            "10.0.0.9/32:22",
            "[2001:db8::4/128]:8443",
            "[2001:db8::5/128]:443",
            "unix:/run/a.sock",
        ],
        "{text}"
    );
}

#[test]
fn client_config_allows_ranges_sockets_and_shared_services() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.yaml");
    let secret = burrow::wire::secret::Secret::generate().encode();
    let other = burrow::wire::secret::Secret::generate().encode();
    std::fs::write(
        &path,
        format!(
            "listen: 7835\nclients:\n  home:\n    secret: {secret}\n  backup:\n    secret: {other}\nservices:\n  game:\n    client: home\n    bind: 30000-30010\n    target: 30000-30010\n  db:\n    client: home\n    bind: 5433\n    target: unix:/run/postgresql/.s.PGSQL.5432\n  web:\n    clients: [backup, home]\n    bind: 8080\n    target: 192.168.1.20:80\n"
        ),
    )
    .unwrap();
    let home = client_config(
        Some("home"),
        &path,
        Some("t.example"),
        Format::Yaml,
        &ProcessEnv,
    )
    .unwrap()
    .text;
    for entry in [
        "127.0.0.1/32:30000-30010",
        "unix:/run/postgresql/.s.PGSQL.5432",
        "192.168.1.20/32:80",
    ] {
        assert!(home.contains(entry), "{entry} missing from:\n{home}");
    }
    let written = dir.path().join("client.yaml");
    std::fs::write(&written, &home).unwrap();
    assert!(matches!(load(&written), Loaded::Client(..)));
    let backup = client_config(
        Some("backup"),
        &path,
        Some("t.example"),
        Format::Yaml,
        &ProcessEnv,
    )
    .unwrap()
    .text;
    assert!(backup.contains("192.168.1.20/32:80"), "{backup}");
    assert!(!backup.contains("30000"), "{backup}");
}

#[test]
fn visitor_config_keeps_a_service_named_like_the_socks_forward() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.yaml");
    let s = || burrow::wire::secret::Secret::generate().encode();
    std::fs::write(
        &path,
        format!(
            "listen: 7835\npublic_address: tunnel.example.com\nclients:\n  home:\n    secret: {}\nvisitors:\n  laptop:\n    secret: {}\n    reach:\n      home: any\nservices:\n  home:\n    client: home\n    target: 22\n    visitors: [laptop]\n  home-socks:\n    client: home\n    target: 80\n    visitors: [laptop]\n",
            s(),
            s()
        ),
    )
    .unwrap();
    let out = burrow::init::visitor_config(
        None,
        &path,
        None,
        burrow::config::Format::Yaml,
        &burrow::config::env::ProcessEnv,
    )
    .unwrap();
    assert!(
        out.text.contains("service: home-socks"),
        "the forward for service home-socks was overwritten:\n{}",
        out.text
    );
}

fn server(dir: &Path, client: &str, services: &str) -> std::path::PathBuf {
    let path = dir.join("server.yaml");
    let s = || burrow::wire::secret::Secret::generate().encode();
    std::fs::write(
        &path,
        format!(
            "listen: 7835\npublic_address: tunnel.example.com\nclients:\n  {client}:\n    secret: {}\nvisitors:\n  laptop:\n    secret: {}\n    reach:\n      {client}: any\nservices:\n{services}",
            s(),
            s()
        ),
    )
    .unwrap();
    path
}

fn printed(dir: &Path, path: &Path) -> (String, burrow::config::Outcome) {
    let out = burrow::init::visitor_config(None, path, None, Format::Yaml, &ProcessEnv).unwrap();
    let written = dir.join("visitor.yaml");
    std::fs::write(&written, &out.text).unwrap();
    let loaded = load_file(&written, Some(Role::Visitor), &ProcessEnv).unwrap();
    (out.text, loaded)
}

#[test]
fn the_socks_forward_never_replaces_a_service_forward() {
    let dir = tempfile::tempdir().unwrap();
    let path = server(
        dir.path(),
        "home",
        "  home: { client: home, target: 5432, visitors: [laptop] }\n  home-socks: { client: home, target: 22, visitors: [laptop] }\n",
    );
    let (text, outcome) = printed(dir.path(), &path);
    let Some(Loaded::Visitor(v, _)) = outcome.loaded else {
        panic!("not a visitor config:\n{text}")
    };
    assert_eq!(v.forwards.len(), 3, "{text}");
    assert!(v.forwards.values().any(|f| f.socks), "{text}");
    assert!(
        v.forwards
            .values()
            .any(|f| f.to == burrow::wire::protocol::VisitTarget::service("home-socks")),
        "the home-socks service lost its forward:\n{text}"
    );
}

#[test]
fn printed_client_and_visitor_configs_stay_byte_identical() {
    let dir = tempfile::tempdir().unwrap();
    let server = dir.path().join("server.yaml");
    std::fs::write(
        &server,
        "listen: 7835\npublic_address: tunnel.example.com\nclients:\n  home:\n    secret: [AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=, \"${HOME_SECRET}\"]\n  office:\n    secret: ICEiIyQlJicoKSorLC0uLzAxMjM0NTY3ODk6Ozw9Pj8=\n  lab:\n    secret: QEFCQ0RFRkdISUpLTE1OT1BRUlNUVVZXWFlaW1xdXl8=\nvisitors:\n  laptop:\n    secret: YGFiY2RlZmdoaWprbG1ub3BxcnN0dXZ3eHl6e3x9fn8=\n    reach:\n      lab: [10.0.0.0/8, 192.168.1.0/24:22]\n  phone:\n    secret: \"${PHONE_SECRET}\"\nservices:\n  web: { client: home, bind: 7840, target: 3000 }\n  db:\n    clients: [home, office]\n    target: 10.0.0.5:5432\n    visitors: [laptop, phone]\n  app:\n    client: office\n    target: localhost:8080-8082\n    bind: 7850-7852\n  sock:\n    client: lab\n    target: unix:/run/app.sock\n    visitors: [laptop]\n  named:\n    client: lab\n    target: printer.lan:631\n    visitors: [phone]\n",
    )
    .unwrap();
    let env = |name: &str| match name {
        "HOME_SECRET" => Some("oKGio6SlpqeoqaqrrK2ur7CxsrO0tba3uLm6u7y9vr8=".to_string()),
        "PHONE_SECRET" => Some("gIGCg4SFhoeIiYqLjI2Oj5CRkpOUlZaXmJmam5ydnp8=".to_string()),
        _ => None,
    };
    let mut out = String::new();
    for format in [Format::Yaml, Format::Json] {
        for name in ["home", "office", "lab"] {
            for server_arg in [None, Some("ws://tunnel.example.com/burrow")] {
                let r = client_config(Some(name), &server, server_arg, format, &env);
                out.push_str(&format!("=== client {name} {format:?} {server_arg:?}\n"));
                match r {
                    Ok(o) => {
                        out.push_str(&format!("{}--- notes\n{}\n", o.text, o.notes.join("\n")))
                    }
                    Err(e) => out.push_str(&format!("error: {e:?}\n")),
                }
            }
        }
        for name in ["laptop", "phone"] {
            let r = burrow::init::visitor_config(Some(name), &server, None, format, &env);
            out.push_str(&format!("=== visitor {name} {format:?}\n"));
            match r {
                Ok(o) => out.push_str(&format!("{}--- notes\n{}\n", o.text, o.notes.join("\n"))),
                Err(e) => out.push_str(&format!("error: {e:?}\n")),
            }
        }
    }
    let out = out.replace(&dir.path().display().to_string(), "DIR");
    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/printed_configs.txt");
    if std::env::var("BURROW_UPDATE_FIXTURES").as_deref() == Ok("1") {
        std::fs::create_dir_all(golden.parent().unwrap()).unwrap();
        std::fs::write(&golden, &out).unwrap();
    }
    let want = std::fs::read_to_string(&golden)
        .expect("the golden file; record it once with BURROW_UPDATE_FIXTURES=1");
    assert_eq!(out, want, "client-config or visitor-config output changed");
}

#[test]
fn a_long_client_name_still_gives_a_valid_visitor_config() {
    let dir = tempfile::tempdir().unwrap();
    let long = "c".repeat(64);
    let path = server(
        dir.path(),
        &long,
        &format!("  {long}: {{ client: {long}, target: 5432, visitors: [laptop] }}\n"),
    );
    let (text, outcome) = printed(dir.path(), &path);
    assert!(
        !outcome.report.has_errors(),
        "{text}\n{}",
        outcome
            .report
            .render_text(burrow::base::style::Style { color: false })
    );
}

#[test]
fn visitor_config_writes_protocol_udp_for_udp_services() {
    let dir = tempfile::tempdir().unwrap();
    let path = visitors_server(
        dir.path(),
        "  laptop:\n    secret: @SECRET@\n",
        "  dns: { client: home, protocol: udp, target: 5353, visitors: [laptop] }\n  mdns: { client: home, target: 5353, visitors: [laptop] }\n  game: { client: home, protocol: udp, target: 27015-27016, visitors: [laptop] }\n",
    );
    let out = burrow::init::visitor_config(None, &path, None, Format::Yaml, &ProcessEnv).unwrap();
    insta::assert_snapshot!(out.text.lines().filter(|l| !l.starts_with("secret:")).collect::<Vec<_>>().join("\n"), @"
    server: tunnel.example.com:7835
    name: laptop
    forwards:
      dns:
        service: dns
        protocol: udp
        local: 5353
      game:
        service: game
        protocol: udp
        local: 27015-27016
      mdns:
        service: mdns
        local: 5353
    ");
    let written = dir.path().join("visitor.yaml");
    std::fs::write(&written, &out.text).unwrap();
    let Loaded::Visitor(v, _) = load(&written) else {
        panic!("not a visitor config")
    };
    assert_eq!(v.forwards["dns"].protocol, burrow_config::Protocol::Udp);
    assert_eq!(v.forwards["game"].protocol, burrow_config::Protocol::Udp);
    assert_eq!(v.forwards["mdns"].protocol, burrow_config::Protocol::Tcp);
}
