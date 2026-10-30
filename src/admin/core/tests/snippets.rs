#![cfg(test)]

use burrow_admin_proto::msg::{AdminLevel, ConnectInfo, ErrorCode, HistoryRequest};
use burrow_admin_proto::ops::Op;
use serde_json::json;

use crate::admin::core::*;

use super::{apply_at, apply_now, core_for, p, session, world};

#[test]
fn secrets_are_revealed_once_and_destructive_changes_need_confirmation() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    let g = core.generate_secret(&s).unwrap();
    let applied = apply_now(
        &core,
        &s,
        &[
            Op::Add {
                path: p(&["clients", "office"]),
                value: json!({}),
            },
            Op::SecretAdd {
                path: p(&["clients", "office", "secret"]),
                handle: g.handle.clone(),
            },
        ],
        &[],
    )
    .unwrap();
    assert_eq!(applied.secrets.len(), 1);
    assert_eq!(applied.secrets[0].fp, g.fp);
    assert!(Secret::parse(&applied.secrets[0].value).is_ok());
    assert!(
        s.pending_secret(&g.handle).is_none(),
        "the handle is used up"
    );
    let e = apply_now(
        &core,
        &s,
        &[Op::Remove {
            path: p(&["clients", "office"]),
        }],
        &[],
    )
    .err()
    .unwrap();
    assert_eq!(e.code, ErrorCode::BadRequest);
    assert_eq!(e.body["confirm"], json!(["office"]));
    apply_now(
        &core,
        &s,
        &[Op::Remove {
            path: p(&["clients", "office"]),
        }],
        &["office"],
    )
    .unwrap();
    let h = core
        .history(&HistoryRequest {
            limit: None,
            diff: None,
            entry: None,
        })
        .unwrap();
    assert_eq!(h.entries.len(), 2);
    assert!(h.entries[0].changes[0].summary.contains("office removed"));
    assert!(
        !serde_json::to_string(&h)
            .unwrap()
            .contains(&applied.secrets[0].value)
    );
}

#[test]
fn the_snippets_after_an_apply_use_the_address_the_admin_connected_to() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    let g = core.generate_secret(&s).unwrap();
    let ops = [
        Op::Add {
            path: p(&["clients", "office"]),
            value: json!({}),
        },
        Op::SecretAdd {
            path: p(&["clients", "office", "secret"]),
            handle: g.handle.clone(),
        },
    ];
    let applied = apply_at(&core, &s, &ops, &[], Some("203.0.113.9:7835")).unwrap();
    let sn = &applied.snippets[0];
    assert!(sn.server_known);
    assert_eq!(sn.server, "203.0.113.9:7835");
    assert!(
        sn.config.starts_with("server: 203.0.113.9:7835\n"),
        "{}",
        sn.config
    );
}

#[test]
fn a_client_config_lists_its_allowed_targets_in_address_order() {
    let w = world();
    let text = w.text.replace(
        "  web: { client: home, bind: 7840, target: 3000 }\n",
        "  web: { client: home, bind: 7840, target: 3000 }\n  a: { client: home, bind: 7841, target: 10.0.0.9:22 }\n  b: { client: home, bind: 7842, target: 10.0.0.10:80 }\n  c: { client: home, bind: 7843, target: 10.0.0.9:8 }\n",
    );
    let core = core_for(&text);
    let sn = snippets::connect_info(
        &core,
        &ConnectInfo {
            kind: "client".into(),
            name: "home".into(),
            address: None,
        },
    )
    .unwrap();
    let allow: Vec<&str> = sn
        .config
        .lines()
        .filter_map(|l| l.trim().strip_prefix("- "))
        .collect();
    assert_eq!(
        allow,
        [
            "10.0.0.9/32:8",
            "10.0.0.9/32:22",
            "10.0.0.10/32:80",
            "127.0.0.1/32:3000"
        ]
    );
}

#[test]
fn a_new_client_comes_with_ready_snippets_and_later_only_with_a_placeholder() {
    let w = world();
    let core = core_for(&w.text.replace(
        "listen: 7835 # tcp\n",
        "listen: 7835 # tcp\npublic_address: tunnel.example.com\n",
    ));
    let s = session(&w, AdminLevel::Write);
    let g = core.generate_secret(&s).unwrap();
    let applied = apply_now(
        &core,
        &s,
        &[
            Op::Add {
                path: p(&["clients", "office"]),
                value: json!({}),
            },
            Op::SecretAdd {
                path: p(&["clients", "office", "secret"]),
                handle: g.handle.clone(),
            },
            Op::Add {
                path: p(&["services", "db"]),
                value: json!({"client": "office", "target": "10.0.0.5:5432", "bind": 7841}),
            },
        ],
        &[],
    )
    .unwrap();
    let value = applied.secrets[0].value.clone();
    assert_eq!(applied.snippets.len(), 1);
    let sn = &applied.snippets[0];
    assert_eq!((sn.kind.as_str(), sn.name.as_str()), ("client", "office"));
    assert!(sn.server_known);
    assert_eq!(sn.server, "tunnel.example.com:7835");
    assert!(sn.with_secret);
    assert_eq!(sn.secret_fp.as_deref(), Some(g.fp.as_str()));
    assert_eq!(
        sn.config,
        format!(
            "server: tunnel.example.com:7835\nname: office\nsecret: {value}\nallow:\n  - 10.0.0.5/32:5432\n"
        )
    );
    assert!(
        sn.env_config.contains("secret: \"${OFFICE_SECRET}\""),
        "{}",
        sn.env_config
    );
    assert!(!sn.env_config.contains(&value));
    assert!(sn.commands.iter().any(|c| c.os == "linux"
        && c.line == "./burrow init client --server tunnel.example.com:7835 --name office"));
    assert!(
        sn.commands
            .iter()
            .any(|c| c.os == "windows" && c.line == r".\burrow.exe service install -c client.yaml")
    );

    let later = snippets::connect_info(
        &core,
        &ConnectInfo {
            kind: "client".into(),
            name: "office".into(),
            address: None,
        },
    )
    .unwrap();
    assert!(!later.with_secret);
    assert!(
        !later.config.contains(&value),
        "a later look never shows the secret"
    );
    assert!(later.config.contains("${OFFICE_SECRET}"));
    let unknown = snippets::connect_info(
        &core,
        &ConnectInfo {
            kind: "visitor".into(),
            name: "nobody".into(),
            address: Some("other.example".into()),
        },
    );
    assert!(unknown.is_err());
    let none = core_for(&w.text);
    let placeholder = snippets::connect_info(
        &none,
        &ConnectInfo {
            kind: "client".into(),
            name: "home".into(),
            address: None,
        },
    )
    .unwrap();
    assert!(!placeholder.server_known);
    assert!(placeholder.config.contains("server: SERVER_ADDRESS"));
}

#[test]
fn the_connect_snippet_of_a_visitor_writes_protocol_udp_for_a_udp_service() {
    let w = world();
    let text = format!(
        "{}  dns: {{ client: home, protocol: udp, target: 53, visitors: [ops] }}\n  db: {{ client: home, target: 53, visitors: [ops] }}\n",
        w.text
    );
    let core = core_for(&text);
    let sn = snippets::connect_info(
        &core,
        &ConnectInfo {
            kind: "visitor".into(),
            name: "ops".into(),
            address: Some("tunnel.example.com".into()),
        },
    )
    .unwrap();
    assert!(
        sn.config
            .contains("  dns:\n    service: dns\n    protocol: udp\n    local: 10053\n"),
        "{}",
        sn.config
    );
    assert!(
        sn.config
            .contains("  db:\n    service: db\n    local: 10053\n"),
        "{}",
        sn.config
    );
}

#[test]
fn read_level_and_read_only_files_cannot_write() {
    let w = world();
    let core = core_for(&w.text);
    let r = session(&w, AdminLevel::Read);
    let e = dispatch(
        &core,
        &r,
        "validate",
        &json!({"base": text::rev(&w.text), "ops": []}),
    )
    .err()
    .unwrap();
    assert_eq!(e.code, ErrorCode::Forbidden);
    assert!(core.generate_secret(&r).is_err());
    let st = core.state(&r).unwrap();
    assert!(!st.file.writable);
    assert_eq!(
        st.file.why_not.unwrap().code,
        burrow_admin_proto::codes::READ_LEVEL
    );
    assert!(st.values["visitors"]["ops"]["secret"]["$secret"]["fp"].is_string());
    assert!(
        !serde_json::to_string(&st.values)
            .unwrap()
            .contains(&w.ops.encode())
    );
}
