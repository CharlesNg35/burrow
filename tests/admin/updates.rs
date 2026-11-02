use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::common::hand::{Hand, NEW, OLD, build};
use super::{Admin, World, connect, world};

fn hand(w: &World) -> Hand {
    let server = if super::common::ws_mode() {
        format!("ws://{}/burrow", w.addr)
    } else {
        w.addr.to_string()
    };
    Hand::new(&server, &w.home)
}

async fn until(a: &mut Admin, what: &str, ok: impl Fn(&Value) -> bool) -> Value {
    let start = Instant::now();
    loop {
        let status = a.event("status").await;
        let home = status["clients"]
            .as_array()
            .and_then(|c| c.iter().find(|c| c["name"] == "home").cloned())
            .unwrap_or_default();
        if ok(&home) {
            return home;
        }
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "no {what}: {home}"
        );
    }
}

fn latest() -> burrow_admin_proto::updates::LatestRelease {
    burrow_admin_proto::updates::LatestRelease {
        version: Some(NEW.into()),
        ..Default::default()
    }
}

async fn start(hand: &mut Hand, w: &World) -> Admin {
    burrow::admin::adapters::use_latest(latest, latest);
    hand.supervise();
    let mut a = connect(w, "ops", &w.ops, 1, 1).await.unwrap();
    a.call("subscribe", json!({"status_ms": 200}))
        .await
        .unwrap();
    until(&mut a, "report of the old version", |c| {
        c["build"]["version"] == OLD
    })
    .await;
    a
}

async fn ask(a: &mut Admin) -> Value {
    a.call(
        "update_clients",
        json!({"clients": ["home"], "version": NEW}),
    )
    .await
    .unwrap()
}

fn same(path: &Path, bytes: &[u8]) -> bool {
    std::fs::read(path).is_ok_and(|b| b == bytes)
}

#[tokio::test]
async fn a_client_started_by_hand_updates_itself_and_reconnects_with_the_new_version() {
    let w = world().await;
    let mut hand = hand(&w);
    let new = build(&[("VERSION", NEW)]);
    hand.releases.publish(NEW, &new, false);
    let mut a = start(&mut hand, &w).await;
    assert_eq!(ask(&mut a).await["asked"], json!(["home"]));
    let home = until(&mut a, "the new version", |c| {
        c["update"]["state"] == "done"
    })
    .await;
    assert_eq!(home["build"]["version"], NEW);
    assert_eq!(
        home["update"]["by"].as_str().unwrap().split(' ').next(),
        Some("ops")
    );
    assert!(same(&hand.binary, &new));
    assert!(same(
        &hand.binary.with_extension("previous"),
        &build(&[("VERSION", OLD)])
    ));
    assert_eq!(hand.record()["outcome"], "done");
    let older = a
        .call(
            "update_clients",
            json!({"clients": ["home"], "version": OLD}),
        )
        .await
        .unwrap_err();
    assert!(
        older.message.contains("only to the latest release"),
        "{older:?}"
    );
    let again = ask(&mut a).await;
    assert!(
        again["skipped"][0]["why"]
            .as_str()
            .unwrap()
            .contains("already runs"),
        "{again}"
    );
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn a_download_that_does_not_match_its_checksum_changes_nothing() {
    let w = world().await;
    let mut hand = hand(&w);
    hand.releases
        .publish(NEW, &build(&[("VERSION", NEW)]), true);
    let mut a = start(&mut hand, &w).await;
    ask(&mut a).await;
    let home = until(&mut a, "a failure", |c| c["update"]["state"] == "failed").await;
    assert!(
        home["update"]["detail"]
            .as_str()
            .unwrap()
            .contains("SHA-256"),
        "{home}"
    );
    assert!(same(&hand.binary, &build(&[("VERSION", OLD)])));
    assert_eq!(home["build"]["version"], OLD);
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn a_new_version_that_does_not_connect_is_rolled_back() {
    let w = world().await;
    let mut hand = hand(&w);
    hand.releases.publish(
        NEW,
        &build(&[("VERSION", NEW), ("BREAK", "connect")]),
        false,
    );
    let mut a = start(&mut hand, &w).await;
    ask(&mut a).await;
    let home = until(&mut a, "a rollback", |c| {
        c["update"]["state"] == "rolled-back"
    })
    .await;
    assert!(
        home["update"]["detail"]
            .as_str()
            .unwrap()
            .contains("did not connect"),
        "{home}"
    );
    assert_eq!(home["build"]["version"], OLD);
    assert!(same(&hand.binary, &build(&[("VERSION", OLD)])));
    assert_eq!(hand.record()["outcome"], "rolled-back");
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn a_new_version_that_keeps_crashing_is_rolled_back_by_the_next_start() {
    let w = world().await;
    let mut hand = hand(&w);
    hand.releases
        .publish(NEW, &build(&[("VERSION", NEW), ("BREAK", "start")]), false);
    let mut a = start(&mut hand, &w).await;
    ask(&mut a).await;
    let home = until(&mut a, "a rollback", |c| {
        c["update"]["state"] == "rolled-back"
    })
    .await;
    assert!(
        home["update"]["detail"]
            .as_str()
            .unwrap()
            .contains("stopped 2 times"),
        "{home}"
    );
    assert!(same(&hand.binary, &build(&[("VERSION", OLD)])));
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn a_release_that_is_not_published_is_refused_by_the_client() {
    let w = world().await;
    let mut hand = hand(&w);
    let mut a = start(&mut hand, &w).await;
    ask(&mut a).await;
    let home = until(&mut a, "a failure", |c| c["update"]["state"] == "failed").await;
    assert!(
        home["update"]["detail"]
            .as_str()
            .unwrap()
            .contains("no burrow release was found"),
        "{home}"
    );
    assert!(same(&hand.binary, &build(&[("VERSION", OLD)])));
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn a_release_that_does_not_accept_the_config_changes_nothing() {
    let w = world().await;
    let mut hand = hand(&w);
    hand.releases
        .publish(NEW, &build(&[("VERSION", NEW), ("BREAK", "config")]), false);
    let mut a = start(&mut hand, &w).await;
    ask(&mut a).await;
    let home = until(&mut a, "a failure", |c| c["update"]["state"] == "failed").await;
    assert!(
        home["update"]["detail"]
            .as_str()
            .unwrap()
            .contains("does not accept"),
        "{home}"
    );
    assert!(same(&hand.binary, &build(&[("VERSION", OLD)])));
    let _ = w.stop.send(true);
}
