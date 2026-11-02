use super::*;
use crate::instance::publish::Entry;
use crate::metrics::Status;

fn found(config: &str, pid: u32) -> Found {
    Found {
        entry: Entry {
            config: PathBuf::from(config),
            pid,
            port: 1,
            token: String::new(),
        },
        status: Status::default(),
    }
}

#[test]
fn with_several_servers_and_none_named_nothing_is_asked_and_each_is_listed() {
    let refused = one(vec![
        found("/etc/burrow/a.yaml", 10),
        found("/etc/burrow/b.yaml", 11),
    ])
    .err()
    .unwrap();
    let text = refused.to_string();
    assert!(
        text.contains(
            "2 burrow servers run on this machine; pick one with --service NAME or -c FILE"
        ),
        "{text}"
    );
    assert!(text.contains("/etc/burrow/a.yaml (pid 10)"), "{text}");
    assert!(text.contains("/etc/burrow/b.yaml (pid 11)"), "{text}");
    assert_eq!(
        one(vec![found("/etc/burrow/a.yaml", 10)])
            .unwrap()
            .entry
            .pid,
        10
    );
}

#[test]
fn a_visitor_or_client_config_is_refused_with_where_to_go_instead() {
    let dir = tempfile::tempdir().unwrap();
    let visitor = dir.path().join("visitor.yaml");
    std::fs::write(&visitor, "server: 127.0.0.1:7000\nname: ops\nsecret: x\nforwards:\n  web:\n    bind: 127.0.0.1:8080\n").unwrap();
    let client = dir.path().join("client.yaml");
    std::fs::write(&client, "server: 127.0.0.1:7000\nname: home\nsecret: x\n").unwrap();
    let v = wanted(&Pick::Config(visitor)).err().unwrap().to_string();
    assert!(
        v.contains("is a visitor") && v.contains("burrow mcp's update_clients"),
        "{v}"
    );
    let c = wanted(&Pick::Config(client)).err().unwrap().to_string();
    assert!(c.contains("is a client"), "{c}");
}

#[test]
fn the_plan_names_who_is_asked_and_why_others_are_not() {
    let reply = UpdateClientsReply {
        to: "0.2.3".into(),
        asked: vec!["home".into(), "nas".into()],
        skipped: vec![burrow_admin_proto::updates::Skipped {
            client: "pi".into(),
            why: "remote updates are off in its config (update: remote: true in client.yaml)"
                .into(),
        }],
    };
    assert_eq!(
        plan(&reply),
        "pi: not asked: remote updates are off in its config (update: remote: true in client.yaml)\nupdate home, nas to 0.2.3, one at a time, stopping at the first that does not come back\n"
    );
}

fn update_of(names: &[&str], all: bool) -> Update {
    Update {
        names: names.iter().map(|n| n.to_string()).collect(),
        all,
        yes: true,
    }
}

fn previewed(asked: &[&str]) -> UpdateClientsReply {
    UpdateClientsReply {
        to: "0.2.3".into(),
        asked: asked.iter().map(|n| n.to_string()).collect(),
        skipped: Vec::new(),
    }
}

#[test]
fn the_confirm_step_sends_all_for_all_and_the_previewed_names_otherwise() {
    let many: Vec<String> = (0..500).map(|i| format!("client-{i}")).collect();
    let names: Vec<&str> = many.iter().map(String::as_str).collect();
    let all = confirmation(&update_of(&[], true), &previewed(&names));
    assert!(all.all && all.clients.is_empty() && !all.dry_run);
    assert_eq!(all.version.as_deref(), Some("0.2.3"));
    let some = confirmation(&update_of(&["a", "b", "c"], false), &previewed(&["a", "c"]));
    assert!(!some.all);
    assert_eq!(some.clients, ["a", "c"]);
}

#[test]
fn json_names_every_skipped_client_even_when_no_one_is_asked() {
    let reply = UpdateClientsReply {
        to: "0.2.3".into(),
        asked: Vec::new(),
        skipped: vec![burrow_admin_proto::updates::Skipped {
            client: "home".into(),
            why: "it is offline".into(),
        }],
    };
    let v: serde_json::Value =
        serde_json::from_str(&outcome_json(&reply, &Default::default())).unwrap();
    assert_eq!(v["asked"], serde_json::json!([]));
    assert_eq!(v["skipped"][0]["why"], "it is offline");
    assert_eq!(v["results"], serde_json::json!({}));
}

#[test]
fn a_slow_poll_is_tried_again_and_only_a_gone_server_or_a_minute_of_failures_ends_the_watch() {
    let start = Instant::now();
    let mut polls = Polls::default();
    assert!(polls.failed(Fetch::Failed, start).is_none());
    assert!(
        polls
            .failed(Fetch::Failed, start + Duration::from_secs(30))
            .is_none()
    );
    polls.answered();
    assert!(
        polls
            .failed(Fetch::Failed, start + Duration::from_secs(40))
            .is_none()
    );
    assert!(
        polls
            .failed(Fetch::Failed, start + Duration::from_secs(90))
            .is_none()
    );
    assert!(
        polls
            .failed(Fetch::Failed, start + Duration::from_secs(101))
            .is_some()
    );
    let mut gone = Polls::default();
    let why = gone.failed(Fetch::Gone, start).unwrap();
    assert!(why.contains("stopped"), "{why}");
}
