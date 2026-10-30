use super::*;

fn p(s: &[&str]) -> Path {
    s.iter().map(|x| x.to_string()).collect()
}

fn secret(fp: &str) -> Value {
    json!({"$secret": {"fp": fp, "src": "literal"}})
}

fn base() -> Value {
    json!({
        "listen": 7835,
        "clients": {"home": {"secret": secret("aaaa0001")}},
        "visitors": {"ops": {"secret": secret("bbbb0002"), "admin": "write"}},
        "services": {"web": {"client": "home", "bind": 7840, "target": 3000}}
    })
}

#[test]
fn edits_become_minimal_ops() {
    let mut c = Changeset::new(base());
    assert!(!c.dirty());
    c.set(&p(&["services", "web", "bind"]), json!("127.0.0.1:7841"));
    c.set(
        &p(&["services", "ssh"]),
        json!({"client": "home", "target": 22}),
    );
    c.remove(&p(&["listen"]));
    let ops = c.ops();
    assert_eq!(
        ops,
        vec![
            Op::Set {
                path: p(&["services", "web", "bind"]),
                value: json!("127.0.0.1:7841")
            },
            Op::Add {
                path: p(&["services", "ssh"]),
                value: json!({"client": "home", "target": 22})
            },
            Op::Remove {
                path: p(&["listen"])
            },
        ]
    );
    assert!(c.modified(&p(&["services", "web", "bind"])));
    assert!(!c.modified(&p(&["services", "web", "target"])));
    assert_eq!(c.changed_entities(), 3);
    c.set(&p(&["pool", "min_idle"]), json!(9));
    assert_eq!(c.changed_entities(), 3, "server settings count once");
    c.reset();
    assert!(!c.dirty());
}

#[test]
fn removing_the_last_entry_of_a_new_group_leaves_no_empty_group() {
    let mut c = Changeset::new(json!({"visitors": {"lap": {"secret": "x"}}}));
    c.set(&p(&["visitors", "lap", "reach", "home"]), json!("any"));
    assert!(c.dirty());
    c.remove_pruned(&p(&["visitors", "lap", "reach", "home"]));
    assert!(!c.dirty(), "{:?}", c.ops());
    c.set(&p(&["pool", "min_idle"]), json!(8));
    c.remove_pruned(&p(&["pool", "min_idle"]));
    assert!(!c.dirty(), "{:?}", c.ops());
    c.set(&p(&["health_check", "interval_secs"]), json!(5));
    c.set(&p(&["health_check", "interval_secs"]), Value::Null);
    assert!(c.dirty(), "clearing a field keeps a group set on purpose");
    let mut kept = Changeset::new(json!({"pool": {"min_idle": 2}}));
    kept.remove_pruned(&p(&["pool", "min_idle"]));
    assert_eq!(
        kept.draft(),
        &json!({"pool": {}}),
        "a group the base has stays"
    );
}

#[test]
fn a_new_client_added_to_services_counts_as_one_change() {
    let mut c = Changeset::new(json!({"clients": {"home": {}}, "services": {
        "web": {"client": "home", "target": 80},
        "db": {"client": "home", "target": 5432}
    }}));
    c.set(&p(&["clients", "example"]), json!({}));
    c.set(&p(&["services", "web", "client"]), Value::Null);
    c.set(
        &p(&["services", "web", "clients"]),
        json!(["home", "example"]),
    );
    c.set(&p(&["services", "db", "client"]), Value::Null);
    c.set(
        &p(&["services", "db", "clients"]),
        json!(["home", "example"]),
    );
    assert_eq!(c.changed_entities(), 1);
    c.set(&p(&["services", "db", "target"]), json!(5433));
    assert_eq!(c.changed_entities(), 2);
    c.rename(&p(&["clients", "home"]), "house");
    assert_eq!(c.changed_entities(), 4, "web now carries two changes");
    assert_eq!(c.renamed_from("clients", "house").as_deref(), Some("home"));
    assert_eq!(c.renamed_from("visitors", "house"), None);
    assert_eq!(c.renamed_to("clients", "home").as_deref(), Some("house"));
}

#[test]
fn secrets_travel_as_handles_and_fingerprints() {
    let mut c = Changeset::new(base());
    c.set(&p(&["clients", "office"]), json!({}));
    c.add_secret(&p(&["clients", "office", "secret"]), "s1", "cccc0003");
    c.add_secret(&p(&["clients", "home", "secret"]), "s2", "dddd0004");
    let ops = c.ops();
    assert_eq!(
        ops,
        vec![
            Op::Add {
                path: p(&["clients", "office"]),
                value: json!({})
            },
            Op::SecretAdd {
                path: p(&["clients", "home", "secret"]),
                handle: "s2".into()
            },
            Op::SecretAdd {
                path: p(&["clients", "office", "secret"]),
                handle: "s1".into()
            },
        ]
    );
    let mut r = Changeset::new(base());
    r.add_secret(&p(&["clients", "home", "secret"]), "s3", "eeee0005");
    r.remove_secret(&p(&["clients", "home", "secret"]), "aaaa0001");
    assert_eq!(
        r.ops(),
        vec![
            Op::SecretAdd {
                path: p(&["clients", "home", "secret"]),
                handle: "s3".into()
            },
            Op::SecretRemove {
                path: p(&["clients", "home", "secret"]),
                fp: "aaaa0001".into()
            },
        ]
    );
    assert!(!format!("{:?}", r.ops()).contains("$secret"));
}

#[test]
fn a_rename_is_one_op_and_carries_references() {
    let mut c = Changeset::new(base());
    assert!(c.rename(&p(&["clients", "home"]), "house"));
    assert_eq!(c.draft()["services"]["web"]["client"], "house");
    c.set(&p(&["services", "web", "target"]), json!(3001));
    let ops = c.ops();
    assert_eq!(
        ops[0],
        Op::Rename {
            path: p(&["clients", "home"]),
            to: "house".into(),
            refs: true
        }
    );
    assert_eq!(
        ops[1],
        Op::Set {
            path: p(&["services", "web", "target"]),
            value: json!(3001)
        }
    );
    assert_eq!(ops.len(), 2);
    assert!(!c.rename(&p(&["clients", "nope"]), "x"));
}

#[test]
fn a_renamed_login_service_takes_its_sites_along_in_one_op() {
    let mut doc = base();
    doc["web"] = json!({"forward_auth": {"sso": {"service": "web", "path": "/auth"}}});
    doc["services"]["wiki"] = json!({"client": "home", "target": 80, "web": {"hosts": ["w.example.com"], "forward_auth": "sso"}});
    let mut c = Changeset::new(doc);
    let at = p(&["web", "forward_auth", "sso"]);
    assert!(c.rename(&at, "authelia"));
    assert_eq!(
        c.draft()["services"]["wiki"]["web"]["forward_auth"],
        "authelia"
    );
    assert!(c.draft()["web"]["forward_auth"].get("authelia").is_some());
    assert_eq!(
        c.ops(),
        [Op::Rename {
            path: at.clone(),
            to: "authelia".into(),
            refs: true
        }]
    );
    assert_eq!(c.renamed_from("web", "authelia"), None);
    let mut snapshot = json!({"web": {"forward_auth": {"sso": {"path": "/auth"}}}});
    rename_under(&mut snapshot, &[], &at, "authelia");
    assert!(snapshot["web"]["forward_auth"].get("authelia").is_some());
    let mut web = json!({"forward_auth": {"sso": {"path": "/auth"}}});
    rename_under(&mut web, &p(&["web"]), &at, "authelia");
    assert!(web["forward_auth"].get("authelia").is_some());
    let mut other = json!({"x": 1});
    rename_under(&mut other, &p(&["services", "wiki"]), &at, "authelia");
    assert_eq!(other, json!({"x": 1}));
    assert!(!c.rename(&p(&["web", "https"]), "x"), "not an entity");
}

#[test]
fn a_kept_draft_moves_onto_a_new_base_without_undoing_hand_edits() {
    let mut c = Changeset::new(base());
    c.set(&p(&["services", "web", "target"]), json!(3002));
    c.add_secret(&p(&["clients", "home", "secret"]), "s1", "ffff0006");
    let mut newer = base();
    newer["clients"]["office"] = json!({"secret": secret("9999aaaa")});
    c.rebase(newer.clone());
    assert_eq!(c.base(), &newer);
    assert_eq!(
        c.draft()["clients"]["office"],
        newer["clients"]["office"],
        "the hand edit stays"
    );
    let ops = c.ops();
    assert_eq!(ops.len(), 2, "{ops:?}");
    assert!(ops.contains(&Op::Set {
        path: p(&["services", "web", "target"]),
        value: json!(3002)
    }));
    assert!(ops.contains(&Op::SecretAdd {
        path: p(&["clients", "home", "secret"]),
        handle: "s1".into()
    }));
    let markers = c.draft()["clients"]["home"]["secret"].to_string();
    assert!(markers.contains("ffff0006"), "{markers}");
}

#[test]
fn environment_values_are_left_alone_unless_replaced() {
    let mut b = base();
    b["listen"] = json!({"$env": {"raw": "${PORT}", "vars": ["PORT"]}});
    let mut c = Changeset::new(b);
    c.set(&p(&["services", "web", "target"]), json!(1));
    assert_eq!(c.ops().len(), 1);
    c.set(&p(&["listen"]), json!(7836));
    assert!(c.ops().contains(&Op::Set {
        path: p(&["listen"]),
        value: json!(7836)
    }));
}

struct Rng(u64);

impl Rng {
    fn next(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }
}

fn random_value(r: &mut Rng, depth: u32) -> Value {
    match r.next(if depth == 0 { 4 } else { 6 }) {
        0 => json!(r.next(100)),
        1 => json!(["a", "b", "c"][r.next(3) as usize]),
        2 => json!(r.next(2) == 0),
        3 => json!([r.next(9), r.next(9)]),
        _ => {
            let mut m = Map::new();
            for _ in 0..r.next(4) {
                m.insert(format!("k{}", r.next(5)), random_value(r, depth - 1));
            }
            Value::Object(m)
        }
    }
}

fn random_path(r: &mut Rng) -> Path {
    (0..=r.next(3)).map(|_| format!("k{}", r.next(5))).collect()
}

#[test]
fn replaying_the_ops_on_the_base_gives_the_draft() {
    let cases: u64 = std::env::var("BURROW_CHANGESET_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3000);
    let mut r = Rng(0x9e37_79b9_7f4a_7c15);
    for case in 0..cases {
        let base = Value::Object(match random_value(&mut r, 3) {
            Value::Object(m) => m,
            other => Map::from_iter([("k0".to_string(), other)]),
        });
        let mut c = Changeset::new(base.clone());
        for _ in 0..=r.next(5) {
            let path = random_path(&mut r);
            if r.next(3) == 0 {
                c.remove(&path);
            } else {
                c.set(&path, random_value(&mut r, 2));
            }
        }
        let ops = c.ops();
        assert_eq!(
            &apply(&base, &ops),
            c.draft(),
            "case {case}: base {base} ops {ops:?}"
        );
        assert_eq!(ops.is_empty(), &base == c.draft(), "case {case}");
    }
}

#[test]
fn ops_between_counts_field_changes() {
    let a = json!({"bind": 1, "target": 2, "allow_from": ["x"]});
    let b = json!({"bind": 1, "target": 3, "allow_from": ["x", "y"], "mode": "balance"});
    let ops = ops_between(&a, &b);
    assert_eq!(ops.len(), 3, "{ops:?}");
    assert!(ops_between(&a, &a).is_empty());
}

#[test]
fn a_renamed_client_keeps_its_place_in_the_draft_reach() {
    let mut c = Changeset::new(json!({
        "clients": {"home": {}, "office": {}},
        "visitors": {"lap": {"reach": {"home": "any", "office": "any"}}}
    }));
    assert!(c.rename(&["clients".to_string(), "home".to_string()], "home2"));
    let keys: Vec<&String> = c.draft()["visitors"]["lap"]["reach"]
        .as_object()
        .unwrap()
        .keys()
        .collect();
    assert_eq!(keys, ["home2", "office"]);
}

#[test]
fn a_new_password_becomes_password_set_and_a_stored_hash_stays_untouched() {
    let site = |auth: Value| json!({"services": {"site": {"web": {"basic_auth": auth}}}});
    let path = |u: &str| {
        ["services", "site", "web", "basic_auth", u]
            .map(String::from)
            .to_vec()
    };
    let base = site(json!({"alice": {"$hash": {}}}));
    let mut c = Changeset::new(base.clone());
    c.set_password(&path("bob"), "p3");
    assert_eq!(
        c.ops(),
        [Op::PasswordSet {
            path: path("bob"),
            handle: "p3".into()
        }]
    );
    let mut first = Changeset::new(json!({"services": {"site": {"web": {}}}}));
    first.set_password(&path("alice"), "p4");
    let ops = first.ops();
    assert_eq!(
        ops.last(),
        Some(&Op::PasswordSet {
            path: path("alice"),
            handle: "p4".into()
        })
    );
    assert!(
        !burrow_admin_proto::serde_json::to_string(&ops)
            .unwrap()
            .contains("$new_password")
    );
    let mut again = Changeset::new(base);
    again.set_password(&path("alice"), "p5");
    assert_eq!(again.ops().len(), 1);
}
