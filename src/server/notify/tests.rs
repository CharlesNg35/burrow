use super::*;
use std::collections::BTreeSet;

use burrow_config::NotifyEvent;
use watch::Kind;

pub(super) fn letter(event: NotifyEvent, resolved: bool) -> Letter {
    Letter {
        message: Message {
            event,
            resolved,
            kind: Kind::Client,
            name: "home".into(),
            title: "Client home is offline".into(),
            text: "Client \"home\" lost its connection.".into(),
        },
        server: Some("tunnel.example.com".into()),
        at: 1_793_448_000,
    }
}

#[test]
fn the_server_is_named_by_the_host_of_its_public_address() {
    assert_eq!(server_name(None), None);
    for (given, host) in [
        ("tunnel.example.com", "tunnel.example.com"),
        ("tunnel.example.com:7835", "tunnel.example.com"),
        ("wss://tunnel.example.com/burrow", "tunnel.example.com"),
        ("[2001:db8::1]:7835", "2001:db8::1"),
        ("203.0.113.9", "203.0.113.9"),
    ] {
        assert_eq!(server_name(Some(given)).as_deref(), Some(host), "{given}");
    }
    let l = letter(NotifyEvent::ClientOffline, false);
    assert_eq!(l.title(), "[tunnel.example.com] Client home is offline");
    let none = Letter { server: None, ..l };
    assert_eq!(none.title(), "burrow: Client home is offline");
}

#[test]
fn a_failure_is_logged_once_and_a_success_clears_it() {
    let mut b = Board::default();
    b.failed("ntfy ntfy.sh", "refused".into());
    let since = b.failing.as_ref().unwrap().1;
    b.failed("ntfy ntfy.sh", "timed out".into());
    let (why, again) = b.failing.clone().unwrap();
    assert_eq!((why.as_str(), again), ("timed out", since));
    b.worked("ntfy ntfy.sh");
    assert!(b.failing.is_none());
    assert_eq!(b.sent, 1);
}

#[tokio::test]
async fn a_message_that_never_goes_through_is_given_up_after_its_tries() {
    let (tx, rx) = mpsc::channel(QUEUE);
    let board = Arc::new(Mutex::new(Board::default()));
    board.lock().unwrap().waiting = 1;
    tx.send(letter(NotifyEvent::ClientOffline, false))
        .await
        .unwrap();
    drop(tx);
    let shown = ("webhook x".to_string(), Duration::from_micros(10));
    deliver(Err("no route".into()), rx, board.clone(), shown).await;
    let b = board.lock().unwrap();
    assert_eq!((b.sent, b.dropped, b.waiting), (0, 1, 0));
    assert_eq!(b.failing.as_ref().unwrap().0, "no route");
}

#[tokio::test]
async fn a_destination_gets_only_the_events_it_asked_for_and_a_full_queue_drops() {
    let n = Notifier::default();
    let target = NotifyTarget {
        via: crate::config::settings::NotifyVia::Webhook("https://127.0.0.1:1/x".into()),
        events: BTreeSet::from([NotifyEvent::ClientBack]),
    };
    n.sync(std::slice::from_ref(&target));
    n.post(&letter(NotifyEvent::ClientOffline, false));
    assert_eq!(n.status()[0].waiting, 0);
    for _ in 0..QUEUE + 3 {
        n.post(&letter(NotifyEvent::ClientBack, true));
    }
    let s = &n.status()[0];
    assert_eq!((s.waiting, s.dropped), (QUEUE as u64, 3));
    assert_eq!(s.name, "webhook 127.0.0.1");
    n.sync(std::slice::from_ref(&target));
    assert_eq!(
        n.status()[0].dropped,
        3,
        "the same destination keeps its worker"
    );
    n.sync(&[]);
    assert!(n.status().is_empty());
}
