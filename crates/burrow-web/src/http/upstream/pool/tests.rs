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
    store.take(key).map(|(c, ..)| c.id)
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
        key("blog").private(7, IpAddr::from([198, 51, 100, 7])),
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

#[test]
fn a_private_key_names_the_visitor_and_ignores_the_pick() {
    let ip = |last: u8| IpAddr::from([198, 51, 100, last]);
    let mine = key("blog").private(7, ip(1));
    assert_eq!(mine.answered(Some(9)), mine, "the pick never moves it");
    let elsewhere = Key {
        way: 2,
        ..key("blog")
    }
    .private(7, ip(1));
    assert_eq!(elsewhere, mine, "whichever client the request picked");
    assert_ne!(
        key("blog").private(7, ip(2)),
        mine,
        "another visitor on the same connection"
    );
    assert_ne!(key("blog").private(8, ip(1)), mine);
}

struct Frames(usize);

impl hyper::body::Body for Frames {
    type Data = bytes::Bytes;
    type Error = crate::http::BoxError;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<bytes::Bytes>, Self::Error>>> {
        if self.0 == 0 {
            return std::task::Poll::Ready(None);
        }
        self.0 -= 1;
        let data = bytes::Bytes::from_static(b"x");
        std::task::Poll::Ready(Some(Ok(hyper::body::Frame::data(data))))
    }

    fn is_end_stream(&self) -> bool {
        self.0 == 0
    }
}

#[tokio::test]
async fn an_answer_that_never_waits_is_listed_for_the_reload_check_from_its_first_frame() {
    use http_body_util::BodyExt;
    let pool = Pool::new();
    let (ours, _app) = tokio::io::duplex(1024);
    let (tx, task) = hyper::client::conn::http1::handshake::<_, crate::http::Body>(
        hyper_util::rt::TokioIo::new(ours),
    )
    .await
    .unwrap();
    let task = tokio::spawn(async move {
        let _ = task.await;
    })
    .abort_handle();
    let home = Home::H1 {
        pool: pool.clone(),
        key: key("blog"),
        client: client(),
        conn: H1 {
            tx,
            ending: None,
            task: task.clone(),
            read: Arc::default(),
        },
        epoch: pool.epoch(),
    };
    let from = crate::Visitor {
        addr: SocketAddr::from(([198, 51, 100, 7], 4000)),
        local: SocketAddr::from(([192, 0, 2, 1], 443)),
        host: "blog.example.com".into(),
    };
    let ticket = pool.ticket(
        &Arc::from("blog"),
        from,
        crate::Ask::Site,
        Cutter::Connection(task),
    );
    let body = Frames(3).boxed();
    let mut back = Back::new(body, home, (crate::Lease::new(()), ticket), None);
    assert!(back.frame().await.is_some());
    assert_eq!(
        pool.busy.len(),
        1,
        "a slow visitor's answer can be cut on reload"
    );
    drop(back);
    assert_eq!(pool.busy.len(), 0);
}
