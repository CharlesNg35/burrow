use std::sync::atomic::{AtomicBool, Ordering};

use super::*;
use crate::Ending;

#[derive(Clone)]
struct Fake {
    id: u32,
    closed: Arc<AtomicBool>,
}

impl AppConn for Fake {
    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    fn is_ready(&self) -> bool {
        !self.is_closed()
    }

    fn ending(&self) -> Option<Ending> {
        None
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }
}

fn conn(id: u32) -> Fake {
    Fake {
        id,
        closed: Arc::default(),
    }
}

fn key(service: &str) -> Key {
    Key {
        service: Arc::from(service),
        way: 1,
        site: 1,
        bound: Bound::Shared,
    }
}

fn client() -> Arc<str> {
    Arc::from("home")
}

fn taken(store: &Store<Fake>, key: &Key) -> Option<u32> {
    store.take(key).map(|(c, _)| c.id)
}

#[test]
fn the_most_recently_used_connection_is_taken_first() {
    let store = Store::default();
    for id in 1..=3 {
        store.put(key("blog"), client(), conn(id));
    }
    assert_eq!(taken(&store, &key("blog")), Some(3));
    assert_eq!(taken(&store, &key("blog")), Some(2));
    store.put(key("blog"), client(), conn(4));
    assert_eq!(taken(&store, &key("blog")), Some(4));
    assert_eq!(taken(&store, &key("blog")), Some(1));
    assert_eq!(taken(&store, &key("blog")), None);
    assert_eq!(store.len(), 0);
}

#[test]
fn a_connection_the_app_closed_is_dropped_instead_of_taken() {
    let store = Store::default();
    let (old, dead) = (conn(1), conn(2));
    store.put(key("blog"), client(), old);
    store.put(key("blog"), client(), dead.clone());
    dead.close();
    assert_eq!(taken(&store, &key("blog")), Some(1));
    let gone = conn(3);
    gone.close();
    store.put(key("blog"), client(), gone);
    assert_eq!(store.len(), 0, "a closed connection is never parked");
}

#[test]
fn each_field_of_the_key_keeps_connections_apart() {
    let store = Store::default();
    store.put(key("blog"), client(), conn(1));
    let addr = |p: u16| SocketAddr::from(([198, 51, 100, 7], p));
    let others = [
        key("shop"),
        Key {
            way: 2,
            ..key("blog")
        },
        Key {
            site: 2,
            ..key("blog")
        },
        Key {
            bound: Bound::Visitor(addr(1000), addr(443)),
            ..key("blog")
        },
        key("blog").private(7),
    ];
    for other in &others {
        assert_eq!(taken(&store, other), None, "{other:?}");
    }
    assert_eq!(taken(&store, &key("blog")), Some(1));
}

#[tokio::test(start_paused = true)]
async fn idle_connections_close_after_60_s_or_5_s_while_their_service_is_crowded() {
    let fakes: Store<Fake> = Store::default();
    fakes.put(key("blog"), client(), conn(1));
    fakes.put(key("shop"), client(), conn(2));
    tokio::time::advance(SWEEP).await;
    fakes.sweep(&|s: &str| if s == "shop" { SWEEP } else { IDLE });
    assert_eq!(taken(&fakes, &key("shop")), None, "crowded: gone after 5 s");
    tokio::time::advance(IDLE - SWEEP - Duration::from_secs(1)).await;
    fakes.sweep(&|_: &str| IDLE);
    assert_eq!(fakes.len(), 1, "nothing goes before 60 s");
    tokio::time::advance(Duration::from_secs(1)).await;
    fakes.sweep(&|_: &str| IDLE);
    assert_eq!(fakes.len(), 0);
}

#[test]
fn eviction_takes_the_services_and_the_clients_idle_connections() {
    let store = Store::default();
    store.put(key("blog"), client(), conn(1));
    store.put(key("shop"), client(), conn(2));
    store.put(key("wiki"), Arc::from("office"), conn(3));
    assert_eq!(
        store.evict("blog", "elsewhere").len(),
        0,
        "no endings on fakes"
    );
    assert_eq!(store.len(), 2);
    store.evict("nothing", "home");
    assert_eq!(taken(&store, &key("wiki")), Some(3));
    assert_eq!(store.len(), 0);
}

#[test]
fn a_reload_closes_the_idle_connections_whose_key_is_no_longer_current() {
    let store = Store::default();
    let (old, kept) = (conn(1), conn(2));
    store.put(key("blog"), client(), old.clone());
    store.put(key("shop"), client(), kept.clone());
    store.retain(&|k: &Key| &*k.service == "shop");
    assert!(old.is_closed());
    assert!(!kept.is_closed());
    assert_eq!(taken(&store, &key("shop")), Some(2));
}

#[test]
fn the_way_that_answered_files_the_connection() {
    let k = key("blog");
    assert_eq!(k.answered(None), k);
    assert_eq!(k.answered(Some(1)), k);
    assert_eq!(k.answered(Some(9)).way, 9);
}
