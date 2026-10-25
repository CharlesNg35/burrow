use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::*;

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
    let t = Instant::now();
    assert_eq!(a.check(&u, Some(&right), me).await, Verdict::Granted);
    let first = t.elapsed();
    let t = Instant::now();
    assert_eq!(a.check(&u, Some(&right), me).await, Verdict::Granted);
    assert!(t.elapsed() * 5 < first, "{:?} vs {first:?}", t.elapsed());
    let changed = users(&[("alice", &hash_password(b"other"))]);
    assert_eq!(a.check(&changed, Some(&right), me).await, Verdict::Wrong);
}

#[tokio::test]
async fn an_unknown_user_costs_about_as_much_as_a_wrong_password() {
    let a = Access::default();
    let u = users(&[("alice", &hash_password(b"s3cret"))]);
    let _ = hash::dummy();
    let mut known = Duration::ZERO;
    let mut unknown = Duration::ZERO;
    for i in 0..3u8 {
        let t = Instant::now();
        let me = ip(&format!("192.0.2.{}", 10 + i));
        a.check(&u, Some(&header("alice", "wrong")), me).await;
        known += t.elapsed();
        let t = Instant::now();
        a.check(&u, Some(&header("mallory", "wrong")), me).await;
        unknown += t.elapsed();
    }
    let (lo, hi) = (known.min(unknown), known.max(unknown));
    assert!(hi < lo * 2, "known {known:?} unknown {unknown:?}");
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
async fn a_flood_of_wrong_passwords_runs_two_checks_at_once_and_the_rest_get_busy() {
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
    let started = Instant::now();
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
    let mut busy = 0;
    for t in tasks {
        let (v, took) = t.await.unwrap();
        assert!(took < SLOT_WAIT + Duration::from_secs(2), "{took:?}");
        match v {
            Verdict::Busy => busy += 1,
            Verdict::Wrong => {}
            other => panic!("{other:?}"),
        }
    }
    assert!(busy > 0);
    assert_eq!(a.slots.available_permits(), SLOTS);
    assert!(started.elapsed() < Duration::from_secs(10));
}
