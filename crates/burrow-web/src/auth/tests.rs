use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use super::*;

#[derive(Default)]
pub(super) struct Work {
    running: AtomicUsize,
    peak: AtomicUsize,
    real: AtomicUsize,
    dummy: AtomicUsize,
}

pub(super) struct Counted<'a>(&'a Work);

impl Work {
    pub(super) fn enter(&self, dummy: bool) -> Counted<'_> {
        let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        let kind = if dummy { &self.dummy } else { &self.real };
        kind.fetch_add(1, Ordering::SeqCst);
        Counted(self)
    }

    fn checks(&self) -> (usize, usize) {
        (
            self.real.load(Ordering::SeqCst),
            self.dummy.load(Ordering::SeqCst),
        )
    }
}

impl Drop for Counted<'_> {
    fn drop(&mut self) {
        self.0.running.fetch_sub(1, Ordering::SeqCst);
    }
}

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

fn users(list: &[(&str, &str)]) -> Users {
    let list: Vec<BasicUser> = list
        .iter()
        .map(|(u, h)| BasicUser {
            user: (*u).into(),
            hash: (*h).into(),
        })
        .collect();
    Users::build("test", &list)
}

fn header(user: &str, pw: &str) -> Vec<u8> {
    basic::encode(user, pw.as_bytes()).into_bytes()
}

#[tokio::test]
async fn the_right_password_passes_and_a_wrong_or_missing_one_does_not() {
    let a = Access::default();
    let h = bcrypt::hash("pw:with:colons", 4).unwrap();
    let u = users(&[("bob", &h)]);
    let me = ip("192.0.2.1");
    let right = header("bob", "pw:with:colons");
    assert_eq!(a.check(&u, Some(&right), me).await, Verdict::Granted);
    let wrong = header("bob", "nope");
    assert_eq!(a.check(&u, Some(&wrong), me).await, Verdict::Wrong);
    assert_eq!(a.check(&u, None, me).await, Verdict::Wrong);
    assert_eq!(a.check(&u, Some(b"Basic !!"), me).await, Verdict::Wrong);
    let other = header("eve", "pw:with:colons");
    assert_eq!(a.check(&u, Some(&other), me).await, Verdict::Wrong);
}

#[tokio::test]
async fn a_cached_success_skips_argon2_and_a_changed_hash_checks_again() {
    let a = Access::default();
    let u = users(&[("alice", &hash_password(b"s3cret"))]);
    let me = ip("192.0.2.2");
    let right = header("alice", "s3cret");
    assert_eq!(a.check(&u, Some(&right), me).await, Verdict::Granted);
    assert_eq!(a.work.checks(), (1, 0));
    assert_eq!(a.check(&u, Some(&right), me).await, Verdict::Granted);
    assert_eq!(a.work.checks(), (1, 0), "the second login is a cache hit");
    let changed = users(&[("alice", &hash_password(b"other"))]);
    assert_eq!(a.check(&changed, Some(&right), me).await, Verdict::Wrong);
    assert_eq!(a.work.checks(), (2, 0));
}

#[tokio::test]
async fn an_unknown_user_costs_one_argon2id_check_like_a_wrong_password() {
    let a = Access::default();
    let u = users(&[("alice", &hash_password(b"s3cret"))]);
    let me = ip("192.0.2.10");
    assert_eq!(
        a.check(&u, Some(&header("alice", "wrong")), me).await,
        Verdict::Wrong
    );
    assert_eq!(a.work.checks(), (1, 0));
    assert_eq!(
        a.check(&u, Some(&header("mallory", "wrong")), me).await,
        Verdict::Wrong
    );
    assert_eq!(
        a.work.checks(),
        (1, 1),
        "the unknown user is checked against the dummy"
    );
    let Stored::Argon2(dummy) = hash::dummy() else {
        panic!("the dummy is argon2id");
    };
    let Some((_, Some(Stored::Argon2(real)))) = u.0.get("alice") else {
        panic!("alice has an argon2id hash");
    };
    assert_eq!(dummy.algorithm, real.algorithm);
    assert_eq!(
        dummy.params, real.params,
        "the dummy costs what burrow passwd writes"
    );
    assert_eq!(a.check(&u, None, me).await, Verdict::Wrong);
    assert_eq!(a.work.checks(), (1, 1), "no header, no check");
}

#[tokio::test]
async fn ten_wrong_passwords_then_429_even_for_the_right_one() {
    let a = Access::default();
    let h = bcrypt::hash("pw", 4).unwrap();
    let u = users(&[("bob", &h)]);
    let me = ip("2001:db8:7:7::1");
    for _ in 0..9 {
        assert_eq!(
            a.check(&u, Some(&header("bob", "x")), me).await,
            Verdict::Wrong
        );
    }
    let tenth = a.check(&u, Some(&header("bob", "x")), me).await;
    assert!(matches!(tenth, Verdict::Wait(_)), "{tenth:?}");
    let same_64 = ip("2001:db8:7:7::99");
    let right = header("bob", "pw");
    assert!(matches!(
        a.check(&u, Some(&right), same_64).await,
        Verdict::Wait(_)
    ));
    let other = ip("2001:db8:7:8::1");
    assert_eq!(a.check(&u, Some(&right), other).await, Verdict::Granted);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_flood_of_wrong_passwords_runs_at_most_two_checks_at_once_and_the_rest_get_busy() {
    let a = Arc::new(Access::default());
    let costly = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(hash::ARGON2_MAX_MEMORY_KIB, hash::ARGON2_MAX_TIME, 1, None).unwrap(),
    );
    let h = argon2::password_hash::PasswordHasher::hash_password(&costly, b"s3cret")
        .unwrap()
        .to_string();
    let u = Arc::new(users(&[("alice", &h)]));
    let tasks: Vec<_> = (0..100u32)
        .map(|i| {
            let (a, u) = (a.clone(), u.clone());
            tokio::spawn(async move {
                let me = IpAddr::from([10, 0, (i / 256) as u8, (i % 256) as u8]);
                let t = Instant::now();
                let v = a.check(&u, Some(&header("alice", "wrong")), me).await;
                (v, t.elapsed())
            })
        })
        .collect();
    let (mut busy, mut wrong) = (0, 0);
    for t in tasks {
        let (v, took) = t.await.unwrap();
        match v {
            Verdict::Busy => {
                busy += 1;
                assert!(took < SLOT_WAIT * 3, "a busy answer waited {took:?}");
            }
            Verdict::Wrong => wrong += 1,
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(a.work.peak.load(Ordering::SeqCst), SLOTS);
    assert_eq!(a.work.checks(), (wrong, 0));
    assert_eq!(busy + wrong, 100);
    assert!(busy > 0);
    assert_eq!(a.slots.available_permits(), SLOTS);
    assert_eq!(a.work.running.load(Ordering::SeqCst), 0);
}
