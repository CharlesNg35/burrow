use super::*;
use burrow_admin_proto::updates::{ClientBuild, RolloutStatus, RunMode};

fn status() -> Status {
    let build = |v: &str| ClientBuild {
        version: v.into(),
        os: "linux".into(),
        arch: "arm64".into(),
        run: RunMode::Service,
        remote: RemoteUpdates::On,
        why_not: None,
    };
    Status {
        clients: vec![
            ClientStatus {
                name: "nas".into(),
                connected: true,
                build: Some(build("0.2.1")),
                available: Some("0.2.2".into()),
                ..ClientStatus::default()
            },
            ClientStatus {
                name: "pi".into(),
                connected: true,
                build: Some(build("0.2.1")),
                available: Some("0.2.2".into()),
                blocked: Some("remote updates are off".into()),
                ..ClientStatus::default()
            },
            ClientStatus {
                name: "box".into(),
                connected: true,
                build: Some(ClientBuild {
                    run: RunMode::ByHand,
                    remote: RemoteUpdates::Off,
                    ..build("0.2.1")
                }),
                available: Some("0.2.2".into()),
                blocked: Some("remote updates are off in its config".into()),
                ..ClientStatus::default()
            },
            ClientStatus {
                name: "old".into(),
                connected: true,
                ..ClientStatus::default()
            },
        ],
        ..Status::default()
    }
}

#[test]
fn a_client_reads_as_its_version_system_and_what_it_can_take() {
    let st = status();
    let nas = of(Some(&st.clone().into()), "nas").unwrap();
    assert_eq!(nas.version, "0.2.1");
    assert_eq!(
        nas.summary(),
        "linux/arm64 · service · remote updates allowed"
    );
    assert_eq!(nas.tag(), Some(("0.2.2 available".into(), Mood::Offer)));
    assert!(nas.ready());
    let pi = of(Some(&st.clone().into()), "pi").unwrap();
    assert!(!pi.ready());
    assert_eq!(pi.tag(), Some(("0.2.2 available".into(), Mood::Quiet)));
    let old = of(Some(&st.clone().into()), "old").unwrap();
    assert!(old.older && old.summary().is_empty());
    assert!(old.older_note().unwrap().contains("sudo burrow update"));
    assert_eq!(of(None, "nas"), None);
    assert_eq!(ready_names(&st), ["nas"]);
}

#[test]
fn a_table_row_reads_short_and_marks_updates_off_only_when_one_is_waiting() {
    let st = status();
    let nas = of(Some(&st.clone().into()), "nas").unwrap();
    assert_eq!(nas.system_short(), "linux/arm64 · service");
    assert!(!nas.refuses());
    let boxed = of(Some(&st.clone().into()), "box").unwrap();
    assert_eq!(boxed.system_short(), "linux/arm64 · started by hand");
    assert_eq!(
        boxed.summary(),
        "linux/arm64 · started by hand · remote updates off in its config"
    );
    assert!(boxed.refuses());
    let mut quiet = st.clone();
    quiet.clients[2].available = None;
    assert!(!of(Some(&quiet.clone().into()), "box").unwrap().refuses());
    assert_eq!(
        of(Some(&st.clone().into()), "old").unwrap().system_short(),
        ""
    );
}

#[test]
fn a_status_push_that_only_ages_an_update_leaves_the_client_view_unchanged() {
    let job = |secs_ago| UpdateStatus {
        id: 1,
        from: "0.2.1".into(),
        to: "0.2.2".into(),
        state: UpdateState::Done,
        secs_ago,
        ..UpdateStatus::default()
    };
    let mut a = status();
    a.clients[0].update = Some(job(3));
    let mut b = status();
    b.clients[0].update = Some(job(4));
    assert_eq!(
        of(Some(&a.clone().into()), "nas"),
        of(Some(&b.clone().into()), "nas")
    );
}

#[test]
fn an_update_in_progress_and_its_end_read_plainly() {
    let mut st = status();
    st.clients[0].update = Some(UpdateStatus {
        id: 1,
        from: "0.2.1".into(),
        to: "0.2.2".into(),
        state: UpdateState::Downloading,
        detail: "x".into(),
        by: "ops (203.0.113.7)".into(),
        secs_ago: 2,
    });
    let nas = of(Some(&st.clone().into()), "nas").unwrap();
    assert_eq!(nas.tag().unwrap().1, Mood::Busy);
    assert_eq!(
        nas.last_update().unwrap().0,
        "0.2.1 → 0.2.2: downloading and checking, asked by ops (203.0.113.7)"
    );
    st.clients[0].update.as_mut().unwrap().state = UpdateState::RolledBack;
    st.rollout = Some(RolloutStatus {
        to: "0.2.2".into(),
        clients: vec!["nas".into(), "pi".into()],
        stopped_at: Some("nas".into()),
        finished: true,
        ..RolloutStatus::default()
    });
    let (line, mood) = rollout_line(&st.clone().into()).unwrap();
    assert_eq!(
        line,
        "Stopped updating to 0.2.2 at nas (rolled back: x); 0 updated"
    );
    assert_eq!(mood, Mood::Bad);
    assert_eq!(
        of(Some(&st.clone().into()), "nas")
            .unwrap()
            .last_update()
            .unwrap()
            .1,
        Mood::Bad
    );
}

fn seen() -> Status {
    Status {
        uptime_secs: 900,
        clients: vec![
            ClientStatus {
                name: "home".into(),
                connected: true,
                from: Some("[2001:db8::1]:51000".into()),
                ..ClientStatus::default()
            },
            ClientStatus {
                name: "office".into(),
                left_secs_ago: Some(120),
                ..ClientStatus::default()
            },
            ClientStatus {
                name: "self".into(),
                connected: true,
                local: true,
                ..ClientStatus::default()
            },
        ],
        ..Status::default()
    }
}

#[test]
fn this_server_is_always_up_and_a_client_is_up_while_connected() {
    let st = seen();
    let reserved = vec!["self".to_string()];
    assert_eq!(
        presence(Some(&st.clone().into()), &reserved, "self"),
        Presence::Here
    );
    assert_eq!(presence(None, &reserved, "self"), Presence::Here);
    assert_eq!(
        presence(Some(&st.clone().into()), &[], "self"),
        Presence::Here
    );
    assert_eq!(
        presence(Some(&st.clone().into()), &reserved, "home"),
        Presence::Online
    );
    assert_eq!(
        presence(Some(&st.clone().into()), &reserved, "office"),
        Presence::Offline
    );
    assert_eq!(
        presence(Some(&st.clone().into()), &reserved, "gone"),
        Presence::Offline
    );
    assert_eq!(presence(None, &reserved, "home"), Presence::Unknown);
    assert!(online(Some(&st.clone().into()), &reserved, "self") && !online(None, &[], "home"));
}

#[test]
fn an_address_reads_as_its_ip_the_same_for_ipv4_and_ipv6() {
    assert_eq!(from_ip("203.0.113.7:51000"), "203.0.113.7");
    assert_eq!(from_ip("[2001:db8::1]:443"), "2001:db8::1");
    assert_eq!(from_ip("[2001:db8::1]"), "2001:db8::1");
    assert_eq!(from_ip("2001:db8::1"), "2001:db8::1");
    assert_eq!(from_ip("203.0.113.7"), "203.0.113.7");
    assert_eq!(from_ip("[::ffff:203.0.113.7]:9"), "203.0.113.7");
    assert_eq!(from_ip("unix socket"), "unix socket");
}

#[test]
fn a_detail_says_where_from_or_since_when() {
    let st = seen();
    let reserved = vec!["self".to_string()];
    assert_eq!(
        detail(Some(&st.clone().into()), &reserved, "home"),
        "online · 2001:db8::1"
    );
    assert_eq!(
        detail(Some(&st.clone().into()), &reserved, "office"),
        "offline"
    );
    assert_eq!(away_text(&st.clone().into(), "office", None), "left 2m ago");
    assert_eq!(
        detail(Some(&st.clone().into()), &reserved, "self"),
        "This server"
    );
    assert_eq!(detail(None, &reserved, "home"), "");
    let home = of(Some(&st.clone().into()), "home").unwrap();
    assert!(home.online && !home.local);
    assert_eq!(home.from.as_deref(), Some("2001:db8::1"));
}

#[test]
fn a_clients_card_offers_a_button_only_when_the_update_is_possible() {
    let mut c = ClientInfo {
        name: "nas".into(),
        version: "0.2.2".into(),
        available: Some("0.2.3".into()),
        ..ClientInfo::default()
    };
    let card = c.card(true);
    assert!(card.can_ask);
    assert_eq!(card.tag.unwrap().0, "0.2.3 available");
    assert!(!c.card(false).can_ask, "a read admin gets no button");
    c.blocked = Some("remote updates are off in its config".into());
    let off = c.card(true);
    assert!(!off.can_ask, "no disabled button");
    let (t, m, why) = off.tag.unwrap();
    assert_eq!((t.as_str(), m), ("0.2.3 available", Mood::Quiet));
    assert!(
        why.contains("off in its config"),
        "the reason is the tooltip"
    );
    c.blocked = None;
    c.failed_recently = true;
    c.update = Some(UpdateStatus {
        from: "0.2.2".into(),
        to: "0.2.3".into(),
        state: UpdateState::Downloading,
        ..UpdateStatus::default()
    });
    let busy = c.card(true);
    assert_eq!(busy.tag.unwrap().0, "updating…");
    assert!(!busy.can_ask);
    c.update.as_mut().unwrap().state = UpdateState::RolledBack;
    c.update.as_mut().unwrap().detail = "did not connect".into();
    let back = c.card(true);
    let (t, m, why) = back.tag.unwrap();
    assert_eq!((t.as_str(), m), ("rolled back", Mood::Offer));
    assert!(
        why.contains("didn't connect to the server within a minute, so nas went back to 0.2.2"),
        "{why}"
    );
    assert!(!back.can_ask, "one message at a time: the result first");
    c.update.as_mut().unwrap().state = UpdateState::Done;
    c.available = None;
    let done = c.card(true);
    assert_eq!(done, Card::default(), "a success leaves no tag");
    let quiet = ClientInfo {
        version: "0.2.3".into(),
        ..ClientInfo::default()
    };
    assert_eq!(quiet.card(true), Card::default(), "nothing to say");
}

#[test]
fn update_all_names_the_clients_it_skips_and_why() {
    let st = status();
    let names: Vec<String> = skipped(&st).into_iter().map(|(n, _)| n).collect();
    for n in &names {
        assert!(
            !ready_names(&st).contains(n),
            "{n} is both asked and skipped"
        );
    }
}
