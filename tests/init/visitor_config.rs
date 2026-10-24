use std::path::Path;

use burrow::base::prompt::NoPrompt;
use burrow::client::State;
use burrow::config::env::ProcessEnv;
use burrow::config::{Format, Loaded};
use burrow::init::{InitOptions, client_config};

use super::common;
use super::common::*;
use super::load;
#[cfg(unix)]
use super::mode;
use super::server_init::Answers;

#[test]
fn visitor_init_asks_for_what_is_missing_and_validates() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("visitor.yaml");
    let secret = burrow::wire::secret::Secret::generate().encode();
    let leaked: &'static str = Box::leak(secret.into_boxed_str());
    let mut p = Answers(vec!["laptop", leaked], Vec::new());
    let opts = InitOptions {
        output: Some(path.clone()),
        server: Some("127.0.0.1:1".into()),
        ..Default::default()
    };
    let mut out = Vec::new();
    burrow::init::init_visitor(&opts, &mut p, &mut out).unwrap();
    assert_eq!(
        p.1,
        [
            "visitor name (as in the server config)",
            "secret (from the server config)"
        ]
    );
    let Loaded::Visitor(v, _) = load(&path) else {
        panic!("not a visitor config")
    };
    assert_eq!(
        (v.name.as_str(), v.server.as_str()),
        ("laptop", "127.0.0.1:1")
    );
    assert!(v.forwards.is_empty());
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains("without forwards: cannot ask the server"),
        "{out}"
    );
    assert!(
        out.contains(&format!(
            "init visitor --update -o {}",
            burrow::base::failure::path_arg(&path)
        )),
        "{out}"
    );
    assert!(
        out.contains("visitor-config laptop -o visitor.yaml"),
        "{out}"
    );
    #[cfg(unix)]
    assert_eq!(mode(&path), 0o600);
    let err = burrow::init::init_visitor(&opts, &mut NoPrompt, &mut Vec::new()).unwrap_err();
    assert!(err.message.contains("already exists"), "{}", err.message);
}

pub(super) fn visitors_server(dir: &Path, visitors: &str, services: &str) -> std::path::PathBuf {
    let path = dir.join("server.yaml");
    let s = || burrow::wire::secret::Secret::generate().encode();
    let mut visitors = visitors.to_string();
    while let Some(i) = visitors.find("@SECRET@") {
        visitors.replace_range(i..i + "@SECRET@".len(), &s());
    }
    std::fs::write(
        &path,
        format!(
            "listen: 7835\npublic_address: tunnel.example.com\nclients:\n  home:\n    secret: {}\nvisitors:\n{visitors}services:\n{services}",
            s()
        ),
    )
    .unwrap();
    path
}

#[test]
fn visitor_config_lists_the_visitors_services() {
    let dir = tempfile::tempdir().unwrap();
    let path = visitors_server(
        dir.path(),
        "  laptop:\n    secret: @SECRET@\n    reach:\n      home: any\n",
        "  db: { client: home, target: 5432, visitors: [laptop] }\n  pg2: { client: home, target: \"10.0.0.2:5432\", visitors: [laptop] }\n  ssh: { client: home, target: 22, visitors: [laptop] }\n  web: { client: home, bind: 7840, target: 3000 }\n",
    );
    let out = burrow::init::visitor_config(None, &path, None, Format::Yaml, &ProcessEnv).unwrap();
    insta::assert_snapshot!(out.text.lines().filter(|l| !l.starts_with("secret:")).collect::<Vec<_>>().join("\n"), @"
    server: tunnel.example.com:7835
    name: laptop
    forwards:
      db:
        service: db
        local: 5432
      pg2:
        service: pg2
        local: 5433
      ssh:
        service: ssh
        local: 10022
      home:
        client: home
        socks: true
        local: 1080
    ");
    assert_eq!(out.notes.len(), 1, "{:?}", out.notes);
    assert!(out.notes[0].starts_with("127.0.0.1:1080 is a SOCKS5 proxy"));
    let written = dir.path().join("visitor.yaml");
    std::fs::write(&written, &out.text).unwrap();
    let Loaded::Visitor(v, _) = load(&written) else {
        panic!("not a visitor config")
    };
    assert_eq!(v.forwards.len(), 4);
}

#[test]
fn visitor_config_suggests_a_socks_target_inside_reach() {
    let dir = tempfile::tempdir().unwrap();
    for (reach, target) in [
        ("[192.168.150.0/25]", "192.168.150.1:22"),
        ("[\"10.0.0.7:8080-8090\"]", "10.0.0.7:8080"),
        ("[\"127.0.0.0/8:5432\"]", "127.0.0.1:5432"),
        ("any", "HOST:PORT"),
    ] {
        let path = visitors_server(
            dir.path(),
            &format!("  laptop:\n    secret: @SECRET@\n    reach:\n      home: {reach}\n"),
            "  web: { client: home, bind: 7840, target: 3000 }\n",
        );
        let out =
            burrow::init::visitor_config(None, &path, None, Format::Yaml, &ProcessEnv).unwrap();
        assert!(
            out.notes[0].contains(&format!("--target {target} ")),
            "{reach}: {:?}",
            out.notes
        );
    }
}

#[test]
fn visitor_config_gives_socket_only_reach_a_forward_per_socket_instead_of_socks() {
    let dir = tempfile::tempdir().unwrap();
    let path = visitors_server(
        dir.path(),
        "  laptop:\n    secret: @SECRET@\n    reach:\n      home: [\"unix:/run/docker.sock\", \"unix:/run/apps/*\"]\n",
        "  web: { client: home, bind: 7840, target: 3000 }\n",
    );
    let out = burrow::init::visitor_config(None, &path, None, Format::Yaml, &ProcessEnv).unwrap();
    assert!(!out.text.contains("socks"), "{}", out.text);
    assert!(
        out.text.contains(
            "home-docker:\n    client: home\n    target: unix:/run/docker.sock\n    local: 10000"
        ),
        "{}",
        out.text
    );
    assert!(
        out.notes.iter().all(|n| !n.contains("SOCKS")),
        "{:?}",
        out.notes
    );
    let written = dir.path().join("visitor.yaml");
    std::fs::write(&written, &out.text).unwrap();
    let Loaded::Visitor(v, _) = load(&written) else {
        panic!("not a visitor config")
    };
    assert_eq!(v.forwards.len(), 1);
}

#[test]
fn visitor_config_explains_what_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let path = visitors_server(
        dir.path(),
        "  laptop:\n    secret: @SECRET@\n  idle:\n    secret: @SECRET@\n",
        "  db: { client: home, target: 5432, visitors: [laptop] }\n",
    );
    let make = |name: Option<&str>| {
        burrow::init::visitor_config(name, &path, None, Format::Yaml, &ProcessEnv)
    };
    let err = make(None).unwrap_err();
    assert_eq!(
        err.message,
        format!("{} defines 2 visitors; which one?", path.display())
    );
    assert!(
        err.hint.unwrap().contains("visitor-config idle -c"),
        "the example uses a real name"
    );
    let err = make(Some("idle")).unwrap_err();
    assert_eq!(
        err.message,
        format!(
            "visitor \"idle\" cannot reach anything in {}",
            path.display()
        )
    );
    let err = make(Some("lapto")).unwrap_err();
    assert_eq!(
        err.hint.unwrap(),
        "did you mean \"laptop\"? defined visitors: idle, laptop"
    );
    assert!(
        make(Some("laptop"))
            .unwrap()
            .text
            .contains("server: tunnel.example.com:7835")
    );
}

#[test]
fn visitor_config_writes_a_config_for_an_admin_only_visitor() {
    let dir = tempfile::tempdir().unwrap();
    let path = visitors_server(
        dir.path(),
        "  laptop:\n    secret: @SECRET@\n  ops:\n    secret: @SECRET@\n    admin: write\n",
        "  db: { client: home, target: 5432, visitors: [laptop] }\n",
    );
    let out =
        burrow::init::visitor_config(Some("ops"), &path, None, Format::Yaml, &ProcessEnv).unwrap();
    assert!(out.text.contains("name: ops"), "{}", out.text);
    assert!(
        out.notes.iter().any(|n| n.contains("admin -c FILE")),
        "{:?}",
        out.notes
    );
}

#[test]
fn visitor_config_keeps_an_env_secret() {
    let dir = tempfile::tempdir().unwrap();
    let path = visitors_server(
        dir.path(),
        "  laptop:\n    secret: ${BURROW_TEST_LAPTOP_SECRET}\n",
        "  db: { client: home, target: 5432, visitors: [laptop] }\n",
    );
    let env = |name: &str| {
        (name == "BURROW_TEST_LAPTOP_SECRET")
            .then(|| burrow::wire::secret::Secret::generate().encode())
    };
    let out = burrow::init::visitor_config(None, &path, None, Format::Yaml, &env).unwrap();
    assert!(
        out.text
            .contains("secret: \"${BURROW_TEST_LAPTOP_SECRET}\""),
        "{}",
        out.text
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn printed_visitor_config_reaches_the_private_service() {
    let dir = tempfile::tempdir().unwrap();
    let port = common::free_port();
    let echo = echo_server().await;
    let path = visitors_server(
        dir.path(),
        "  laptop:\n    secret: @SECRET@\n",
        &format!("  db: {{ client: home, target: \"{echo}\", visitors: [laptop] }}\n"),
    );
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("listen: 7835", &format!("listen: 127.0.0.1:{port}"));
    std::fs::write(&path, text).unwrap();
    let Loaded::Server(server, _) = load(&path) else {
        panic!()
    };
    let address = format!("127.0.0.1:{port}");
    let write = |name: &str, text: &str| {
        let p = dir.path().join(name);
        std::fs::write(&p, text).unwrap();
        load(&p)
    };
    let client = client_config(None, &path, Some(&address), Format::Yaml, &ProcessEnv).unwrap();
    let Loaded::Client(client, _) = write("client.yaml", &client.text) else {
        panic!()
    };
    let visitor =
        burrow::init::visitor_config(None, &path, Some(&address), Format::Yaml, &ProcessEnv)
            .unwrap();
    let Loaded::Visitor(mut visitor, _) = write("visitor.yaml", &visitor.text) else {
        panic!()
    };
    visitor.forwards.get_mut("db").unwrap().local = "127.0.0.1:0".parse().unwrap();

    let srv = start_server(*server).await;
    let mut cli = start_client(*client).await;
    wait_state(&mut cli.state, State::Connected).await;
    let shared = srv.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 1).await;
    let v = start_visitor(*visitor);
    assert_eq!(
        roundtrip(v.addr("db"), b"from a visitor").await,
        b"from a visitor"
    );
}

#[test]
fn visitor_config_gives_reach_a_socks_proxy_and_ranges_a_local_range() {
    let dir = tempfile::tempdir().unwrap();
    let path = visitors_server(
        dir.path(),
        "  laptop:\n    secret: @SECRET@\n    reach:\n      home: any\n",
        "  game:\n    client: home\n    target: 30000-30010\n    visitors: [laptop]\n",
    );
    let out = burrow::init::visitor_config(None, &path, None, Format::Yaml, &ProcessEnv).unwrap();
    for line in ["local: 30000-30010", "socks: true", "local: 1080"] {
        assert!(
            out.text.contains(line),
            "{line} missing from:\n{}",
            out.text
        );
    }
    assert_eq!(
        out.notes,
        [
            "127.0.0.1:1080 is a SOCKS5 proxy into client \"home\": point an app at it, or forward one target with: burrow visit --client home --target HOST:PORT --local 2222"
        ]
    );
    let written = dir.path().join("visitor.yaml");
    std::fs::write(&written, &out.text).unwrap();
    let Loaded::Visitor(v, _) = load(&written) else {
        panic!("a visitor config")
    };
    assert_eq!(v.forwards["game"].ports, 11);
    assert!(v.forwards["home"].socks);
}
