use super::*;
use crate::status::{EntryStatus, HttpCounts};

fn service(i: usize) -> ServiceStatus {
    ServiceStatus {
        name: format!("svc-{i:05}"),
        protocol: "tcp".into(),
        bind: Some(format!("0.0.0.0:{}", 10_000 + i)),
        target: "127.0.0.1:22".into(),
        clients: vec!["home".into()],
        visitors: vec!["laptop".into()],
        open: 1,
        total: 10,
        bytes_in: 1_000,
        bytes_out: 2_000,
        http: (i % 2 == 0).then(|| HttpCounts {
            responses: [0, 5, 0, 1, 0],
        }),
        entries: vec![EntryStatus {
            client: "home".into(),
            target: None,
            open: 1,
        }],
        ..ServiceStatus::default()
    }
}

fn client(i: usize) -> ClientStatus {
    ClientStatus {
        name: format!("client-{i:04}"),
        connected: i % 3 != 0,
        connected_secs: (i % 3 != 0).then_some(100),
        left_secs_ago: (i % 3 == 0).then_some(50),
        from: (i % 3 != 0).then(|| "10.0.0.1".into()),
        ..ClientStatus::default()
    }
}

fn visitor(i: usize) -> VisitorStatus {
    VisitorStatus {
        name: format!("visitor-{i:04}"),
        open: 0,
        total: 3,
        last_seen_secs: Some(7),
    }
}

fn status(services: usize, clients: usize, visitors: usize) -> Status {
    Status {
        version: "0.2.7".into(),
        uptime_secs: 1_000,
        listening: vec!["clients on tcp 0.0.0.0:7000".into()],
        services: (0..services).map(service).collect(),
        clients: (0..clients).map(client).collect(),
        visitors: (0..visitors).map(visitor).collect(),
        ..Status::default()
    }
}

fn tick(s: &Status, secs: u64) -> Status {
    let mut n = s.clone();
    n.uptime_secs += secs;
    n.clients.iter_mut().for_each(|c| c.age(secs));
    n.visitors.iter_mut().for_each(|v| v.age(secs));
    n
}

fn round_trip(old: &Status, new: &Status) -> StatusDelta {
    let d = diff(old, new);
    let wire: StatusDelta =
        serde_json::from_str(&serde_json::to_string(&d).unwrap()).expect("the delta reads back");
    assert_eq!(wire, d, "the delta survives JSON");
    let mut got = old.clone();
    apply(&mut got, wire).expect("the delta applies");
    assert_eq!(&got, new, "diff then apply gives the new status");
    d
}

#[test]
fn an_unchanged_status_gives_an_empty_delta_even_as_ages_advance() {
    let s = status(50, 10, 10);
    let d = round_trip(&s, &s);
    assert!(d.is_empty() && d.up == 1_000);
    let later = tick(&s, 1);
    let d = round_trip(&s, &later);
    assert!(d.is_empty(), "a second passing is only the uptime: {d:?}");
    assert_eq!(
        serde_json::to_string(&StatusDelta { seq: 9, ..d }).unwrap(),
        r#"{"seq":9,"up":1001}"#
    );
    let d = round_trip(&s, &tick(&s, 3));
    assert!(d.is_empty(), "a skipped tick ages by the uptime difference");
}

#[test]
fn busy_services_send_only_their_counters() {
    let s = status(100, 5, 5);
    let mut n = tick(&s, 1);
    n.services[3].bytes_in += 500;
    n.services[4].http.as_mut().unwrap().responses[1] += 1;
    n.services[7].entries[0].open = 4;
    n.clients[1].open = 9;
    n.clients[2].connected_secs = Some(0);
    n.visitors[0].last_seen_secs = Some(0);
    let d = round_trip(&s, &n);
    let names: Vec<&str> = d.services.counts.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(names, ["svc-00003", "svc-00004", "svc-00007"]);
    assert!(d.services.set.is_empty() && d.services.add.is_empty());
    assert_eq!(d.clients.counts.len(), 2);
    assert_eq!(d.visitors.counts.len(), 1);
    let row = serde_json::to_string(&d.services.counts[0]).unwrap();
    assert_eq!(row, r#"["svc-00003",[1,10,0,1500,2000,null,[1]]]"#);
}

#[test]
fn a_changed_shape_sends_the_whole_row() {
    let s = status(20, 5, 0);
    let mut n = s.clone();
    n.services[2].target = "127.0.0.1:2222".into();
    n.services[5].health = vec![crate::status::HealthStatus {
        client: "home".into(),
        target: None,
        up: false,
        reason: Some("refused".into()),
    }];
    n.clients[1].connected = false;
    let d = round_trip(&s, &n);
    assert_eq!(d.services.set.len(), 2);
    assert_eq!(d.clients.set.len(), 1);
    assert!(d.services.counts.is_empty());
}

#[test]
fn adds_removes_and_renames_keep_the_order() {
    let s = status(10, 4, 4);
    let mut n = s.clone();
    n.services.remove(0);
    n.services.insert(3, service(500));
    n.services.push(service(600));
    n.services[6].name = "renamed".into();
    n.clients.remove(3);
    n.visitors.insert(0, visitor(99));
    let d = round_trip(&s, &n);
    assert_eq!(d.services.gone, ["svc-00000", "svc-00006"]);
    let at: Vec<usize> = d.services.add.iter().map(|(i, _)| *i).collect();
    assert_eq!(at, [3, 6, 10]);
    assert!(d.services.all.is_none());
    assert_eq!(d.clients.gone, ["client-0003"]);
    let mut empty = Status::default();
    let full = status(3, 1, 1);
    apply(&mut empty, diff(&Status::default(), &full)).unwrap();
    assert_eq!(empty, full);
    let mut gone = full.clone();
    apply(&mut gone, diff(&full, &Status::default())).unwrap();
    assert_eq!(gone, Status::default());
}

#[test]
fn a_reorder_or_a_repeated_name_sends_the_whole_list() {
    let s = status(6, 0, 0);
    let mut n = s.clone();
    n.services.swap(1, 4);
    let d = round_trip(&s, &n);
    assert!(d.services.all.is_some());
    let mut dup = s.clone();
    dup.services[2].name = dup.services[1].name.clone();
    assert!(round_trip(&s, &dup).services.all.is_some());
    assert!(round_trip(&dup, &s).services.all.is_some());
}

#[test]
fn head_and_web_go_only_when_they_change() {
    let s = status(2, 0, 0);
    let mut n = s.clone();
    n.handshakes_refused = 4;
    let d = round_trip(&s, &n);
    assert!(d.head.is_some() && d.web.is_none());
    n.web = Some(WebSummary {
        https: "0.0.0.0:443".into(),
        ..WebSummary::default()
    });
    let d = round_trip(&s, &n);
    assert!(d.web.is_some());
    let d = round_trip(&n, &s);
    assert_eq!(d.web.as_deref(), Some(&WebPart { summary: None }));
    let mut c = s.clone();
    c.certificates = vec![CertStatus {
        name: "a.example.com".into(),
        source: crate::status::CertSourceStatus::LocalCa,
        check: None,
        state: crate::status::CertStateStatus::Ok,
        expires: None,
        next_renewal: None,
        last_error: None,
        code: None,
        explanation: None,
        failures: 0,
    }];
    assert_eq!(round_trip(&s, &c).certificates.add.len(), 1);
}

#[test]
fn a_delta_that_does_not_fit_is_refused() {
    let s = status(3, 0, 0);
    let mut n = s.clone();
    n.services[1].bytes_in = 7;
    let d = diff(&s, &n);
    let mut other = status(2, 0, 0);
    other.services[1].name = "else".into();
    assert_eq!(apply(&mut other, d), Err(Mismatch));
    let bad = StatusDelta {
        services: Rows {
            add: vec![(9, service(9))],
            ..Rows::default()
        },
        ..StatusDelta::default()
    };
    assert_eq!(apply(&mut status(1, 0, 0), bad), Err(Mismatch));
}

fn shown(f: Follow) -> Status {
    match f {
        Follow::Show(s) => *s,
        other => panic!("expected a status, got {other:?}"),
    }
}

#[test]
fn the_follower_takes_a_snapshot_then_deltas_and_asks_again_on_a_gap() {
    let mut leader = Leader::default();
    let mut follower = Follower::default();
    let s1 = status(5, 2, 2);
    let u = leader.update(&s1);
    assert!(matches!(u, StatusUpdate::Full(_)) && u.seq() == 1);
    leader.sent(s1.clone());
    let mut cur = shown(follower.take(None, u));
    assert_eq!(cur, s1);
    let mut s2 = tick(&s1, 1);
    s2.services[0].total = 99;
    let u = leader.update(&s2);
    assert!(matches!(u, StatusUpdate::Delta(_)) && u.seq() == 2);
    leader.sent(s2.clone());
    cur = shown(follower.take(Some(&cur), u));
    assert_eq!(cur, s2);
    let s3 = tick(&s2, 1);
    let lost = leader.update(&s3);
    leader.sent(s3.clone());
    let s4 = tick(&s3, 1);
    let u4 = leader.update(&s4);
    leader.sent(s4.clone());
    assert_eq!(lost.seq() + 1, u4.seq());
    assert_eq!(follower.take(Some(&cur), u4.clone()), Follow::Resync);
    assert_eq!(
        follower.take(Some(&cur), u4),
        Follow::Wait,
        "asks once until a snapshot comes"
    );
    let unsent = leader.update(&tick(&s4, 1));
    assert_eq!(unsent.seq(), 5, "an update not sent leaves the sequence");
    let mut fresh = Follower::default();
    assert_eq!(
        fresh.take(None, unsent),
        Follow::Resync,
        "a delta before any snapshot"
    );
    let mut again = Leader::default();
    let u = again.update(&s4);
    again.sent(s4.clone());
    cur = shown(follower.take(Some(&cur), u));
    let s5 = tick(&s4, 1);
    cur = shown(follower.take(Some(&cur), again.update(&s5)));
    assert_eq!(cur, s5);
}

#[test]
fn the_wire_names_the_two_kinds() {
    let full = StatusUpdate::Full(Snapshot {
        seq: 1,
        status: Box::new(Status::default()),
    });
    let v = serde_json::to_value(&full).unwrap();
    assert_eq!(v["full"]["seq"], 1);
    assert!(v["full"]["status"].is_object());
    let d = StatusUpdate::Delta(Box::new(StatusDelta {
        seq: 2,
        up: 5,
        ..StatusDelta::default()
    }));
    assert_eq!(
        serde_json::to_string(&d).unwrap(),
        r#"{"delta":{"seq":2,"up":5}}"#
    );
}

#[test]
#[ignore = "measurement: cargo test -p burrow-admin-proto --release -- --ignored --nocapture"]
fn measure_bytes_and_time_per_tick_at_5000_services() {
    let s = status(5_000, 500, 1_000);
    let mut n = tick(&s, 1);
    for svc in n.services.iter_mut().step_by(10) {
        svc.bytes_in += 1_234;
        svc.total += 1;
    }
    let runs = 20;
    let t = std::time::Instant::now();
    let mut full = Vec::new();
    for _ in 0..runs {
        full = serde_json::to_vec(&serde_json::to_value(&n).unwrap()).unwrap();
    }
    let full_ms = t.elapsed().as_secs_f64() * 1000.0 / f64::from(runs);
    let t = std::time::Instant::now();
    let mut quiet = Vec::new();
    for _ in 0..runs {
        quiet = serde_json::to_vec(&diff(&s, &tick(&s, 1))).unwrap();
    }
    let quiet_ms = t.elapsed().as_secs_f64() * 1000.0 / f64::from(runs);
    let t = std::time::Instant::now();
    let mut busy = Vec::new();
    for _ in 0..runs {
        busy = serde_json::to_vec(&diff(&s, &n)).unwrap();
    }
    let busy_ms = t.elapsed().as_secs_f64() * 1000.0 / f64::from(runs);
    let t = std::time::Instant::now();
    for _ in 0..runs {
        let d: StatusDelta = serde_json::from_slice(&busy).unwrap();
        let mut c = s.clone();
        apply(&mut c, d).unwrap();
    }
    let apply_ms = t.elapsed().as_secs_f64() * 1000.0 / f64::from(runs);
    println!(
        "full: {} B {full_ms:.2} ms; unchanged delta: {} B {quiet_ms:.2} ms; 500 busy services: {} B {busy_ms:.2} ms; parse+clone+apply busy: {apply_ms:.2} ms",
        full.len(),
        quiet.len(),
        busy.len()
    );
}
