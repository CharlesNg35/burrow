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

const FILE: &str = "/etc/burrow/server.yaml";

fn judged(who: &Who, old: &ServerSettings, new: &ServerSettings) -> Vec<Refusal> {
    refusals(
        who,
        &Change {
            running: old,
            base: Base::Valid(old),
            next: new,
            renames: &[],
            path: Path::new(FILE),
        },
    )
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
    let i = impacts(&old, &new, &status, &[], &[], &[]);
    let removed = i.iter().find(|i| i.code == "removes-service").unwrap();
    assert_eq!(
        removed.message,
        "removes service web; port 0.0.0.0:7840 closes; closes 12 open connections on it"
    );
    assert_eq!(removed.confirm.as_deref(), Some("web"));
    let moved = settings(&w.text.replace("bind: 7840", "bind: 7841"));
    let i = impacts(&old, &moved, &status, &[], &[], &[]);
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
    let i = impacts(&old, &new, &Status::default(), &live, &[], &[]);
    assert!(
        i.iter()
            .any(|i| i.code == "disconnects-client" && i.confirm.as_deref() == Some("home")),
        "{i:?}"
    );
    let safe = impacts(&old, &new, &Status::default(), &[], &[], &[]);
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
    assert!(judged(&who, &old, &old).is_empty());
    let read = settings(&w.text.replace("admin: write", "admin: read"));
    let r = judged(&who, &old, &read);
    assert!(r.iter().any(|r| r.code == "self-demoted"));
    let no_listen = settings(
        &w.text
            .replace("listen: 7835 # tcp\n", "websocket: { listen: 7836 }\n"),
    );
    assert!(
        judged(&who, &old, &no_listen)
            .iter()
            .any(|r| r.code == "self-transport")
    );
}

#[test]
fn a_new_self_allow_names_the_reach_connections_it_may_close() {
    let w = world();
    let with_self = |allow: &str| {
        settings(&w.text.replace(
            "services:",
            &format!("self:\n  allow: [{allow}]\nservices:"),
        ))
    };
    let mut old = with_self("127.0.0.0/8");
    let auth = old.visitors.get_mut("ops").unwrap();
    auth.reach
        .insert(SELF.into(), crate::allowlist::AllowList::All);
    let mut new = with_self("127.0.0.1/32");
    new.visitors = old.visitors.clone();
    let status = Status {
        clients: vec![crate::metrics::ClientStatus {
            name: SELF.into(),
            connected: true,
            open: 3,
            local: true,
            ..Default::default()
        }],
        ..Default::default()
    };
    let i = impacts(&old, &new, &status, &[], &[], &[]);
    let changed = i.iter().find(|i| i.code == "changes-self-allow").unwrap();
    assert_eq!(
        changed.message,
        "changes self.allow; of the 3 open connections to this server, those the new list no longer permits close"
    );
    assert_eq!(changed.level, Level::Warning);
    assert!(impacts(&old, &old, &status, &[], &[], &[]).is_empty());
}

struct Team {
    text: String,
    ops: Secret,
    boss: Secret,
    spare: Secret,
    viewer: Secret,
}

fn team() -> Team {
    let w = world();
    let (boss, spare, viewer, plain) = (
        Secret::generate(),
        Secret::generate(),
        Secret::generate(),
        Secret::generate(),
    );
    let text = w.text.replace(
        "\nservices:",
        &format!(
            "  boss:\n    secret: [{}, {}]\n    admin: write\n  viewer:\n    secret: {}\n    admin: read\n    admin_from: [10.0.0.0/8, 2001:db8::/32]\n  plain:\n    secret: {}\n\nservices:",
            boss.encode(),
            spare.encode(),
            viewer.encode(),
            plain.encode()
        ),
    );
    Team {
        text,
        ops: w.ops,
        boss,
        spare,
        viewer,
    }
}

fn ops(t: &Team) -> Who<'_> {
    Who {
        visitor: "ops",
        peer: "203.0.113.7:1".parse().unwrap(),
        secret: &t.ops,
        via_websocket: false,
    }
}

fn codes(r: &[Refusal]) -> Vec<&str> {
    r.iter().map(|r| r.code.as_str()).collect()
}

fn in_file(what: &str) -> String {
    format!(
        "admin access is granted only in the config file: {what}; use burrow edit -c {FILE} or edit the file"
    )
}

#[test]
fn granting_or_widening_admin_power_is_refused_with_its_own_code() {
    let t = team();
    let old = settings(&t.text);
    let who = ops(&t);
    let refused = |text: String| judged(&who, &old, &settings(&text));
    let plain_admin = t.text.replace("  plain:\n", "  plain:\n    admin: write\n");
    let r = refused(plain_admin);
    assert_eq!(codes(&r), ["admin-grant"]);
    assert_eq!(
        r[0].message,
        in_file("visitor \"plain\" would get admin: write")
    );
    let r = refused(t.text.replace("admin: read", "admin: write"));
    assert_eq!(codes(&r), ["admin-upgrade"]);
    assert_eq!(
        r[0].message,
        in_file("visitor \"viewer\" would go from admin: read to admin: write")
    );
    let r = refused(
        t.text
            .replace("    admin_from: [10.0.0.0/8, 2001:db8::/32]\n", ""),
    );
    assert_eq!(codes(&r), ["admin-from-widened"]);
    assert_eq!(
        r[0].message,
        in_file("admin_from of visitor \"viewer\" would be removed, which admits every address")
    );
    let r = refused(
        t.text
            .replace("2001:db8::/32]", "2001:db8::/32, 192.0.2.0/24]"),
    );
    assert_eq!(
        r[0].message,
        in_file(
            "admin_from of visitor \"viewer\" would admit 192.0.2.0/24, which the current list does not"
        )
    );
    assert_eq!(
        codes(&refused(t.text.replace("2001:db8::/32]", "::/0]"))),
        ["admin-from-widened"]
    );
    let fresh = Secret::generate();
    let rotated = t.text.replace(
        &format!("[{}, {}]", t.boss.encode(), t.spare.encode()),
        &format!("[{}, {}]", t.boss.encode(), fresh.encode()),
    );
    let r = refused(rotated);
    assert_eq!(codes(&r), ["admin-secret-added"]);
    assert_eq!(
        r[0].message,
        in_file(&format!(
            "visitor \"boss\" would get the new secret {}",
            fresh.fingerprint()
        ))
    );
    assert!(!r[0].message.contains(&fresh.encode()));
    let added = t.text.replace(
        "\nservices:",
        &format!(
            "  extra:\n    secret: {}\n    admin: read\n\nservices:",
            Secret::generate().encode()
        ),
    );
    let r = refused(added);
    assert_eq!(codes(&r), ["admin-grant"]);
    assert_eq!(
        r[0].message,
        in_file("visitor \"extra\" would get admin: read")
    );
}

#[test]
fn renaming_an_admin_visitor_is_left_to_the_file() {
    let t = team();
    let old = settings(&t.text);
    let new = settings(&t.text.replace("  boss:\n", "  chief:\n"));
    let renames = [Rename {
        path: vec!["visitors".into(), "boss".into()],
        to: "chief".into(),
    }];
    let r = refusals(
        &ops(&t),
        &Change {
            running: &old,
            base: Base::Valid(&old),
            next: &new,
            renames: &renames,
            path: Path::new(FILE),
        },
    );
    assert_eq!(codes(&r), ["admin-grant"]);
    assert_eq!(
        r[0].message,
        format!(
            "an admin visitor is renamed only in the config file: visitor \"boss\" would become \"chief\"; use burrow edit -c {FILE} or edit the file"
        )
    );
    let plain = settings(&t.text.replace("  plain:\n", "  simple:\n"));
    assert!(judged(&ops(&t), &old, &plain).is_empty());
}

#[test]
fn reducing_admin_power_of_others_is_allowed() {
    let t = team();
    let old = settings(&t.text);
    let who = ops(&t);
    let allowed = |text: String| {
        let r = judged(&who, &old, &settings(&text));
        assert!(r.is_empty(), "{text}: {r:?}");
    };
    allowed(t.text.replacen("    admin: write\n  viewer", "  viewer", 1));
    allowed(
        t.text
            .replacen("    admin: write\n  viewer", "    admin: read\n  viewer", 1),
    );
    allowed(t.text.replace("    admin: read\n", ""));
    allowed(t.text.replace(
        "[10.0.0.0/8, 2001:db8::/32]",
        "[10.1.0.0/16, ::ffff:10.2.3.4]",
    ));
    allowed(
        t.text
            .replace("[10.0.0.0/8, 2001:db8::/32]", "[10.0.0.0/9, 10.128.0.0/9]"),
    );
    allowed(t.text.replace(
        &format!("[{}, {}]", t.boss.encode(), t.spare.encode()),
        &t.spare.encode(),
    ));
    let gone = settings(&t.text.replace(
        &format!(
            "  viewer:\n    secret: {}\n    admin: read\n    admin_from: [10.0.0.0/8, 2001:db8::/32]\n",
            t.viewer.encode()
        ),
        "",
    ));
    assert!(!gone.visitors.contains_key("viewer"));
    assert!(judged(&who, &old, &gone).is_empty());
    let plain_secret = t.text.replace(
        "  plain:\n    secret: ",
        &format!("  plain:\n    secret: [{}, ", Secret::generate().encode()),
    );
    let plain_secret = plain_secret.replacen("\n\nservices:", "]\n\nservices:", 1);
    allowed(plain_secret);
}

#[test]
fn the_self_guards_still_decide_for_the_session() {
    let t = team();
    let old = settings(&t.text);
    let who = ops(&t);
    let judge = |text: String| judged(&who, &old, &settings(&text));
    let ops_line = format!("  ops:\n    secret: {}\n", t.ops.encode());
    assert_eq!(
        codes(&judge(t.text.replacen(
            "admin: write\n  boss",
            "admin: read\n  boss",
            1
        ))),
        ["self-demoted"]
    );
    let fresh = Secret::generate();
    assert_eq!(
        codes(&judge(t.text.replace(
            &ops_line,
            &format!(
                "  ops:\n    secret: [{}, {}]\n",
                t.ops.encode(),
                fresh.encode()
            )
        ))),
        ["admin-secret-added"]
    );
    assert_eq!(
        codes(&judge(t.text.replace(
            &ops_line,
            &format!("  ops:\n    secret: {}\n", fresh.encode())
        ))),
        ["self-secret", "admin-secret-added"]
    );
    assert_eq!(
        codes(&judge(t.text.replace(
            &ops_line,
            &format!("{ops_line}    admin_from: [10.0.0.0/8]\n")
        ))),
        ["self-source"]
    );
    assert_eq!(
        codes(&judge(t.text.replace(
            &ops_line,
            &format!("{ops_line}    admin_from: [203.0.113.0/24]\n")
        ))),
        Vec::<&str>::new()
    );
}

#[test]
fn the_base_is_the_file_so_an_unapplied_hand_edit_is_not_a_grant() {
    let t = team();
    let running = settings(&t.text);
    let edited = settings(&t.text.replace("  plain:\n", "  plain:\n    admin: write\n"));
    let later = settings(
        &t.text
            .replace("  plain:\n", "  plain:\n    admin: write\n")
            .replace("target: 3000", "target: 3001"),
    );
    let change = |file: &ServerSettings, next: &ServerSettings| {
        refusals(
            &ops(&t),
            &Change {
                running: &running,
                base: Base::Valid(file),
                next,
                renames: &[],
                path: Path::new(FILE),
            },
        )
    };
    assert!(change(&edited, &later).is_empty());
    assert_eq!(codes(&change(&running, &later)), ["admin-grant"]);
    assert_eq!(codes(&change(&running, &edited)), ["admin-grant"]);
}

#[test]
fn a_grant_brings_no_impact_since_it_is_refused() {
    let t = team();
    let old = settings(&t.text);
    let new = settings(&t.text.replace("  plain:\n", "  plain:\n    admin: write\n"));
    let i = impacts(&old, &new, &Status::default(), &[], &[], &[]);
    assert!(i.iter().all(|i| !i.code.starts_with("admin")), "{i:?}");
}

#[test]
fn services_that_only_change_target_or_clients_share_one_impact() {
    let (a, b) = (
        crate::secret::Secret::generate().encode(),
        crate::secret::Secret::generate().encode(),
    );
    let head = format!(
        "listen: 7835\nclients:\n  a:\n    secret: {a}\n  b:\n    secret: {b}\nservices:\n"
    );
    let old = settings(&format!(
        "{head}  web: {{ client: a, target: 80, bind: 7840 }}\n  db: {{ client: a, target: 5432, bind: 7841 }}\n  ssh: {{ client: a, target: 22, bind: 7842 }}\n"
    ));
    let new = settings(&format!(
        "{head}  web: {{ clients: [a, b], target: 80, bind: 7840 }}\n  db: {{ clients: [a, b], target: 5432, bind: 7841 }}\n  ssh: {{ client: a, target: 22, bind: 7842 }}\n"
    ));
    let changed: Vec<String> = impacts(&old, &new, &Status::default(), &[], &[], &[])
        .into_iter()
        .filter(|i| i.code == "changes-service")
        .map(|i| i.message)
        .collect();
    assert_eq!(
        changed,
        ["services db and web use their new target or clients for new connections; open ones stay"]
    );
}

fn renamed(section: &str, from: &str, to: &str) -> Rename {
    Rename {
        path: vec![section.into(), from.into()],
        to: to.into(),
    }
}

fn twins() -> (String, String) {
    let (c, v, o) = (
        crate::secret::Secret::generate().encode(),
        crate::secret::Secret::generate().encode(),
        crate::secret::Secret::generate().encode(),
    );
    (
        format!(
            "listen: 7835\nclients:\n  home:\n    secret: {c}\nvisitors:\n  ops:\n    secret: {o}\n    admin: write\n  home:\n    secret: {v}\nservices:\n  web: {{ client: home, bind: 7840, target: 3000 }}\n"
        ),
        c,
    )
}

#[test]
fn renaming_a_connected_client_says_it_is_locked_out_until_its_config_says_the_new_name() {
    let (text, c) = twins();
    let old = settings(&text);
    let new = settings(
        &text
            .replace(
                &format!("  home:\n    secret: {c}"),
                &format!("  house:\n    secret: {c}"),
            )
            .replace("client: home", "client: house"),
    );
    let renames = [renamed("clients", "home", "house")];
    let live = [LiveSession {
        client: "home".into(),
        secret_fp: String::new(),
        from: "198.51.100.4:1".into(),
    }];
    let status = Status {
        clients: vec![crate::metrics::ClientStatus {
            name: "home".into(),
            open: 2,
            ..Default::default()
        }],
        ..Default::default()
    };
    let i = impacts(&old, &new, &status, &live, &[], &renames);
    let r = i.iter().find(|i| i.code == "renames").expect("renames");
    assert_eq!(
        r.message,
        "renames client home to house; it is connected and will be disconnected, closing 2 open connections, until its client.yaml says name: house"
    );
    assert_eq!(r.level, Level::Danger);
    assert_eq!(r.confirm.as_deref(), Some("home"));
    assert!(
        i.iter()
            .all(|i| i.code != "removes-client" && i.code != "adds-client"),
        "{i:?}"
    );
    let away = impacts(&old, &new, &Status::default(), &[], &[], &renames);
    let r = away.iter().find(|i| i.code == "renames").expect("renames");
    assert_eq!(
        r.message,
        "renames client home to house; it cannot connect until its client.yaml says name: house"
    );
    assert_eq!(r.level, Level::Warning);
    assert_eq!(r.confirm, None);
}

#[test]
fn renaming_a_bound_service_closes_its_port_and_its_open_connections() {
    let (text, _) = twins();
    let old = settings(&text);
    let new = settings(&text.replace("  web: {", "  site: {"));
    let renames = [renamed("services", "web", "site")];
    let status = Status {
        services: vec![ServiceStatus {
            name: "web".into(),
            open: 3,
            ..Default::default()
        }],
        ..Default::default()
    };
    let i = impacts(&old, &new, &status, &[], &[], &renames);
    let r = i.iter().find(|i| i.code == "renames").expect("renames");
    assert_eq!(
        r.message,
        "renames service web to site; port 0.0.0.0:7840 closes and reopens; closes 3 open connections on it"
    );
    assert_eq!(r.level, Level::Danger);
    assert_eq!(r.confirm.as_deref(), Some("web"));
    assert!(
        i.iter()
            .all(|i| i.code != "removes-service" && i.code != "adds-service"),
        "{i:?}"
    );
    let idle = impacts(&old, &new, &Status::default(), &[], &[], &renames);
    let r = idle.iter().find(|i| i.code == "renames").expect("renames");
    assert_eq!(r.level, Level::Danger, "the port still closes");
    assert_eq!(r.confirm.as_deref(), Some("web"));
}

#[test]
fn renaming_a_visitor_with_open_tunnels_closes_them() {
    let (text, _) = twins();
    let old = settings(&text);
    let renames = [renamed("visitors", "home", "phone")];
    let new = settings(&{
        let mut t = text.clone();
        let at = t.find("visitors:").unwrap();
        let tail = t.split_off(at);
        t + &tail.replacen("  home:\n", "  phone:\n", 1)
    });
    let status = Status {
        visitors: vec![VisitorStatus {
            name: "home".into(),
            open: 1,
            ..Default::default()
        }],
        ..Default::default()
    };
    let i = impacts(&old, &new, &status, &[], &[], &renames);
    let r = i.iter().find(|i| i.code == "renames").expect("renames");
    assert_eq!(
        r.message,
        "renames visitor home to phone; closes 1 open connection; it cannot connect until its visitor.yaml says name: phone"
    );
    assert_eq!(r.level, Level::Danger);
    assert_eq!(r.confirm.as_deref(), Some("home"));
}

#[test]
fn a_rename_hides_only_the_removal_and_addition_of_the_entity_in_its_own_section() {
    let (text, c) = twins();
    let old = settings(&text);
    let mut t = text
        .replace(
            &format!("clients:\n  home:\n    secret: {c}\n"),
            "clients: {}\n",
        )
        .replace("  web: { client: home, bind: 7840, target: 3000 }\n", "");
    let at = t.find("visitors:").unwrap();
    let tail = t.split_off(at);
    t = t + &tail.replacen("  home:\n", "  phone:\n", 1);
    let t = t.replace(
        "services:\n",
        "services:\n  site: { client: x, bind: 7850, target: 3000 }\n",
    );
    let t = t.replace(
        "clients: {}\n",
        &format!("clients:\n  x:\n    secret: {c}\n"),
    );
    let new = settings(&t);
    let renames = [renamed("visitors", "home", "phone")];
    let i = impacts(&old, &new, &Status::default(), &[], &[], &renames);
    let codes: Vec<(&str, Option<&str>)> = i
        .iter()
        .map(|i| (i.code.as_str(), i.confirm.as_deref()))
        .collect();
    assert!(
        codes.contains(&("removes-client", Some("home"))),
        "{codes:?}"
    );
    assert!(
        codes.contains(&("removes-service", Some("web"))),
        "{codes:?}"
    );
    assert!(
        i.iter()
            .any(|i| i.code == "adds-service" && i.message.contains("site")),
        "{i:?}"
    );
    assert!(
        !codes.contains(&("removes-visitor", Some("home"))),
        "{codes:?}"
    );
    assert_eq!(i.iter().filter(|i| i.code == "renames").count(), 1);
}
