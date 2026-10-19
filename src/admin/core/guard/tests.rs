use super::*;
use crate::admin::core::tests::world;
use crate::config::{Format, Loaded, Role, load_str};
use crate::metrics::{ServiceStatus, VisitorStatus};

fn settings(text: &str) -> ServerSettings {
    let nothing = |_: &str| None;
    match load_str(
        "server.yaml",
        text,
        Format::Yaml,
        Some(Role::Server),
        &nothing,
    )
    .loaded
    {
        Some(Loaded::Server(s, _)) => *s,
        _ => panic!("loads: {text}"),
    }
}

#[test]
fn impacts_count_the_connections_a_change_closes() {
    let w = world();
    let old = settings(&w.text);
    let new = settings(&w.text.replace(
        "services:\n  web: { client: home, bind: 7840, target: 3000 }\n",
        "services: {}\n",
    ));
    let status = Status {
        services: vec![ServiceStatus {
            name: "web".into(),
            open: 12,
            ..Default::default()
        }],
        visitors: vec![VisitorStatus {
            name: "ops".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let i = impacts(&old, &new, &status, &[], &[]);
    let removed = i.iter().find(|i| i.code == "removes-service").unwrap();
    assert_eq!(
        removed.message,
        "removes service web; port 0.0.0.0:7840 closes; closes 12 open connections on it"
    );
    assert_eq!(removed.confirm.as_deref(), Some("web"));
    let moved = settings(&w.text.replace("bind: 7840", "bind: 7841"));
    let i = impacts(&old, &moved, &status, &[], &[]);
    assert!(
        i.iter()
            .any(|i| i.code == "rebinds-service" && i.confirm.as_deref() == Some("web"))
    );
}

#[test]
fn removing_the_secret_a_client_is_connected_with_disconnects_it() {
    let w = world();
    let other = Secret::generate();
    let both = w.text.replace(
        &format!("secret: {}\n\nvisitors", w.home.encode()),
        &format!(
            "secret: [{}, {}]\n\nvisitors",
            w.home.encode(),
            other.encode()
        ),
    );
    let old = settings(&both);
    let new = settings(&both.replace(
        &format!("[{}, {}]", w.home.encode(), other.encode()),
        &other.encode(),
    ));
    let live = [LiveSession {
        client: "home".into(),
        secret_fp: w.home.fingerprint(),
        from: "198.51.100.4:1".into(),
    }];
    let i = impacts(&old, &new, &Status::default(), &live, &[]);
    assert!(
        i.iter()
            .any(|i| i.code == "disconnects-client" && i.confirm.as_deref() == Some("home")),
        "{i:?}"
    );
    let safe = impacts(&old, &new, &Status::default(), &[], &[]);
    assert!(!safe.iter().any(|i| i.code == "disconnects-client"));
}

#[test]
fn the_last_writer_and_the_transport_are_kept() {
    let w = world();
    let old = settings(&w.text);
    let who = Who {
        visitor: "ops",
        peer: "203.0.113.7:1".parse().unwrap(),
        secret: &w.ops,
        via_websocket: false,
    };
    assert!(refusals(&who, &old, &old).is_empty());
    let read = settings(&w.text.replace("admin: write", "admin: read"));
    let r = refusals(&who, &old, &read);
    assert!(r.iter().any(|r| r.code == "self-demoted"));
    let no_listen = settings(
        &w.text
            .replace("listen: 7835 # tcp\n", "websocket: { listen: 7836 }\n"),
    );
    assert!(
        refusals(&who, &old, &no_listen)
            .iter()
            .any(|r| r.code == "self-transport")
    );
}
