use std::sync::Arc;

use super::*;

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

fn far() -> Instant {
    Instant::now() + Duration::from_secs(60)
}

async fn hold_now(p: &Permits, who: &str) -> Permit {
    let entry = p.enter().await.arrived();
    p.take(entry, ip(who), far()).await.unwrap()
}

fn waiter(p: &Arc<Permits>, who: &'static str, deadline: Instant) -> tokio::task::JoinHandle<bool> {
    let p = p.clone();
    tokio::spawn(async move {
        let entry = p.enter().await.arrived();
        p.take(entry, ip(who), deadline).await.is_some()
    })
}

async fn settle() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn a_connection_without_a_slot_waits_for_one_until_its_deadline() {
    let p = Arc::new(Permits::new(2));
    let a = hold_now(&p, "203.0.113.7").await;
    let _b = hold_now(&p, "203.0.113.7").await;
    let served = waiter(&p, "203.0.113.7", far());
    let late = waiter(
        &p,
        "203.0.113.8",
        Instant::now() + Duration::from_millis(50),
    );
    settle().await;
    assert_eq!(p.admitted(), 4, "two handshaking and two waiting");
    assert!(
        !late.await.unwrap(),
        "no slot came free before its deadline"
    );
    drop(a);
    assert!(served.await.unwrap(), "a freed slot goes to the waiter");
    assert_eq!(p.admitted(), 1);
}

#[tokio::test(start_paused = true)]
async fn one_address_may_fill_only_its_share_of_a_busy_waiting_room() {
    let p = Arc::new(Permits::new(16));
    let held: Vec<_> = hold_loopback(&p, 16).await;
    let waiters: Vec<_> = (0..40).map(|_| waiter(&p, "203.0.113.7", far())).collect();
    settle().await;
    assert_eq!(p.admitted(), 16 + 32, "a waiter past the share was refused");
    let other = waiter(&p, "198.51.100.20", far());
    settle().await;
    assert_eq!(p.admitted(), 16 + 33, "another address still waits");
    drop(held);
    assert!(other.await.unwrap());
    let mut refused = 0;
    for w in waiters {
        refused += usize::from(!w.await.unwrap());
    }
    assert_eq!(refused, 8);
    assert!(p.waiting.is_empty());
}

async fn hold_loopback(p: &Permits, n: usize) -> Vec<Permit> {
    let mut held = Vec::new();
    for _ in 0..n {
        held.push(hold_now(p, "127.0.0.1").await);
    }
    held
}

#[tokio::test(start_paused = true)]
async fn the_accept_loop_waits_while_the_room_is_full_and_moves_on_after_a_resize() {
    let p = Arc::new(Permits::new(1));
    let mut entries = Vec::new();
    for _ in 0..5 {
        entries.push(p.enter().await);
    }
    let enter = || {
        let p = p.clone();
        tokio::spawn(async move { p.enter().await })
    };
    let next = enter();
    settle().await;
    assert!(!next.is_finished(), "a full room stops the accept loop");
    drop(entries.pop());
    entries.push(next.await.unwrap());
    let blocked = enter();
    settle().await;
    assert!(!blocked.is_finished());
    p.resize(2);
    blocked.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_fast_handshake_never_counts_and_a_stalled_one_counts_against_its_address() {
    let p = Arc::new(Permits::new(16));
    let mut quick = hold_now(&p, "203.0.113.7").await;
    assert_eq!(p.hold(&mut quick, async { 7 }).await, Some(7));
    drop(quick);
    let mut tasks = Vec::new();
    for _ in 0..16 {
        let mut permit = hold_now(&p, "203.0.113.7").await;
        let p = p.clone();
        tasks.push(tokio::spawn(async move {
            let stalled = p.hold(&mut permit, std::future::pending::<()>());
            tokio::time::timeout(Duration::from_secs(3), stalled)
                .await
                .is_err()
        }));
    }
    let mut kept = 0;
    for t in tasks {
        kept += usize::from(t.await.unwrap());
    }
    assert_eq!(
        kept, 8,
        "a busy pool keeps an eighth (at least 8) of stalled ones per address"
    );
    assert!(p.stalled.is_empty());
}

#[tokio::test(start_paused = true)]
async fn loopback_and_an_address_within_its_part_keep_their_stalled_handshakes() {
    let p = Arc::new(Permits::new(16));
    let mut held = Vec::new();
    for who in ["127.0.0.1", "::1", "203.0.113.7"] {
        for _ in 0..4 {
            held.push(hold_now(&p, who).await);
        }
    }
    let mut waited = Vec::new();
    for permit in &mut held {
        let stalled = p.hold(permit, tokio::time::sleep(Duration::from_secs(2)));
        waited.push(stalled.await.is_some());
    }
    assert!(waited.iter().all(|w| *w));
}
