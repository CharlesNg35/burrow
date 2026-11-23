use std::cell::RefCell;

use burrow_admin_logic::live::Live;
use burrow_admin_proto::delta::{Follow, Follower, StatusUpdate};
use burrow_admin_proto::msg::ev;
use burrow_admin_proto::serde::de::DeserializeOwned;
use burrow_admin_proto::serde_json::{self, Value};
use burrow_admin_proto::status::Status;
use leptos::prelude::*;

use crate::store::Store;

thread_local! {
    static FOLLOWER: RefCell<Follower> = RefCell::new(Follower::default());
}

#[derive(burrow_admin_proto::serde::Deserialize)]
#[serde(crate = "burrow_admin_proto::serde")]
struct Event<T> {
    body: T,
}

fn straight<T: DeserializeOwned>(text: &str, name: &str) -> Option<T> {
    let head = format!(r#"{{"ev":"{name}","body":"#);
    text.starts_with(&head)
        .then(|| serde_json::from_str::<Event<T>>(text).ok())
        .flatten()
        .map(|e| e.body)
}

pub(crate) fn restart() {
    FOLLOWER.with(|f| *f.borrow_mut() = Follower::default());
}

pub(crate) fn text(store: Store, text: &str, now_ms: f64) -> bool {
    if let Some(s) = straight::<Status>(text, ev::STATUS) {
        store.take_status(s, now_ms);
        return true;
    }
    if let Some(u) = straight::<StatusUpdate>(text, ev::STATUS_SYNC) {
        follow(store, u, now_ms);
        return true;
    }
    false
}

pub(crate) fn event(store: Store, name: &str, body: Value, now_ms: f64) {
    match name {
        ev::STATUS => {
            if let Ok(s) = serde_json::from_value::<Status>(body) {
                store.take_status(s, now_ms);
            }
        }
        ev::STATUS_SYNC => {
            if let Ok(u) = serde_json::from_value::<StatusUpdate>(body) {
                follow(store, u, now_ms);
            }
        }
        _ => {}
    }
}

fn follow(store: Store, update: StatusUpdate, now_ms: f64) {
    let next = store.status.with_untracked(|cur| {
        FOLLOWER.with(|f| {
            f.borrow_mut()
                .take(cur.as_deref().map(Live::status), update)
        })
    });
    match next {
        Follow::Show(s) => store.take_status(*s, now_ms),
        Follow::Resync => crate::api::subscribe(),
        Follow::Wait => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::delta::{Snapshot, StatusDelta};
    use burrow_admin_proto::msg;

    #[test]
    fn both_status_events_are_read_straight_into_their_types() {
        let st = Status {
            uptime_secs: 7,
            ..Status::default()
        };
        let text = msg::event(msg::ev::STATUS, serde_json::to_value(&st).unwrap()).to_string();
        assert_eq!(
            straight::<Status>(&text, ev::STATUS).map(|s| s.uptime_secs),
            Some(7)
        );
        let other = msg::event("later", serde_json::json!({})).to_string();
        assert!(straight::<Status>(&other, ev::STATUS).is_none());
        let reordered = format!(
            r#"{{"body":{},"ev":"status"}}"#,
            serde_json::to_string(&st).unwrap()
        );
        assert!(
            straight::<Status>(&reordered, ev::STATUS).is_none(),
            "other shapes take the general path"
        );
        let full = StatusUpdate::Full(Snapshot {
            seq: 1,
            status: Box::new(st),
        });
        let text = String::from_utf8(msg::event_bytes(ev::STATUS_SYNC, &full)).unwrap();
        assert_eq!(straight::<StatusUpdate>(&text, ev::STATUS_SYNC), Some(full));
        let delta = StatusUpdate::Delta(Box::new(StatusDelta {
            seq: 2,
            up: 8,
            ..StatusDelta::default()
        }));
        let text = String::from_utf8(msg::event_bytes(ev::STATUS_SYNC, &delta)).unwrap();
        assert!(straight::<Status>(&text, ev::STATUS).is_none());
        assert_eq!(
            straight::<StatusUpdate>(&text, ev::STATUS_SYNC),
            Some(delta)
        );
    }
}
