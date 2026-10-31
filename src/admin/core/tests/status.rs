use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use burrow_config::SELF;

use super::{FakeLive, core_running, world};
use crate::metrics::{ClientStatus, Status};

fn client(name: &str, connected: bool, local: bool) -> ClientStatus {
    ClientStatus {
        name: name.into(),
        connected,
        local,
        ..ClientStatus::default()
    }
}

#[test]
fn the_status_carries_the_allow_list_each_connected_client_sent() {
    let w = world();
    let mut core = core_running(&w.text, &w.text);
    let rules = vec!["127.0.0.1:*".to_string(), "unix:/run/app.sock".to_string()];
    core.live = Box::new(FakeLive {
        text: Arc::new(Mutex::new(w.text.clone())),
        status: Mutex::new(Status {
            clients: vec![
                client("home", true, false),
                client("office", false, false),
                client("lab", true, false),
                client(SELF, true, true),
            ],
            ..Status::default()
        }),
        sessions: Mutex::new(Vec::new()),
        busy: Vec::new(),
        allows: BTreeMap::from([
            ("home".to_string(), rules.clone()),
            ("office".to_string(), vec!["any".to_string()]),
            (SELF.to_string(), vec!["any".to_string()]),
        ]),
    });
    let st = core.status();
    let allow = |n: &str| {
        st.clients
            .iter()
            .find(|c| c.name == n)
            .unwrap()
            .allow
            .clone()
    };
    assert_eq!(allow("home"), Some(rules));
    assert_eq!(allow("office"), None, "a client that is gone shows none");
    assert_eq!(allow("lab"), None, "a session that sent no list yet");
    assert_eq!(allow(SELF), None, "the server's own client has no list");
}

#[test]
fn a_status_without_allow_lists_keeps_its_old_json() {
    let c = client("home", true, false);
    let old = serde_json::json!({
        "name": "home", "connected": true, "connected_secs": null, "from": null, "open": 0,
        "idle_channels": 0, "left_secs_ago": null, "busy_total": 0, "at_limit": false
    });
    assert_eq!(serde_json::to_value(&c).unwrap(), old);
    let any = ClientStatus {
        allow: Some(vec!["any".into()]),
        ..c
    };
    let back: ClientStatus = serde_json::from_value(serde_json::to_value(&any).unwrap()).unwrap();
    assert_eq!(back, any);
}
