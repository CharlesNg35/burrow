use std::net::IpAddr;
use std::num::NonZeroU32;
use std::time::Duration;

use governor::clock::FakeRelativeClock;

use super::*;

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

fn n(v: u32) -> NonZeroU32 {
    NonZeroU32::new(v).unwrap()
}

#[test]
fn a_visitor_gets_rate_plus_burst_then_429_with_a_retry_of_at_least_a_second() {
    let clock = FakeRelativeClock::default();
    let l = Limit::with_clock(n(5), clock.clone());
    for _ in 0..10 {
        assert!(l.check(ip("203.0.113.7")).is_ok());
    }
    let wait = l.check(ip("203.0.113.7")).unwrap_err();
    assert!(wait <= Duration::from_millis(200));
    assert_eq!(retry_after(wait), 1);
    assert!(l.check(ip("203.0.113.8")).is_ok());
    clock.advance(Duration::from_millis(200));
    assert!(l.check(ip("203.0.113.7")).is_ok());
}

#[test]
fn one_ipv6_64_shares_a_budget_and_two_do_not() {
    let l = Limit::with_clock(n(1), FakeRelativeClock::default());
    assert!(l.check(ip("2001:db8:1:1::1")).is_ok());
    assert!(l.check(ip("2001:db8:1:1::2")).is_ok());
    assert!(l.check(ip("2001:db8:1:1:ffff::3")).is_err());
    assert!(l.check(ip("2001:db8:1:2::1")).is_ok());
}

#[test]
fn a_mapped_ipv4_is_the_same_visitor_and_two_ipv4_are_not() {
    let l = Limit::with_clock(n(1), FakeRelativeClock::default());
    assert!(l.check(ip("1.2.3.4")).is_ok());
    assert!(l.check(ip("::ffff:1.2.3.4")).is_ok());
    assert!(l.check(ip("1.2.3.4")).is_err());
    assert!(l.check(ip("1.2.3.5")).is_ok());
    assert!(l.check(ip("::ffff:1.2.3.6")).is_ok());
}

#[test]
fn the_sweep_drops_visitors_whose_budget_refilled() {
    let clock = FakeRelativeClock::default();
    let l = Limit::with_clock(n(10), clock.clone());
    for i in 0..5000u32 {
        let a = std::net::Ipv4Addr::from(0x0a00_0000 + i);
        assert!(l.check(IpAddr::V4(a)).is_ok());
    }
    assert_eq!(l.len(), 5000);
    l.sweep();
    assert_eq!(l.len(), 5000);
    clock.advance(Duration::from_secs(3));
    l.sweep();
    assert_eq!(l.len(), 0);
}

#[test]
fn ten_wrong_passwords_a_minute_then_the_visitor_waits() {
    let clock = FakeRelativeClock::default();
    let f = Failures::with_clock(clock.clone());
    let me = ip("198.51.100.4");
    for _ in 0..9 {
        assert_eq!(f.blocked(me), None);
        assert_eq!(f.failed(me), None);
    }
    assert_eq!(f.blocked(me), None);
    let wait = f.failed(me).expect("the tenth failure empties the budget");
    assert!(wait <= Duration::from_secs(6));
    assert!(f.blocked(me).is_some());
    assert!(f.blocked(ip("198.51.100.5")).is_none());
    clock.advance(wait);
    assert_eq!(f.blocked(me), None);
    assert!(f.failed(me).is_some());
}

#[test]
fn wrong_passwords_count_per_64_and_the_sweep_empties_the_lists() {
    let clock = FakeRelativeClock::default();
    let f = Failures::with_clock(clock.clone());
    for i in 0..10u32 {
        let _ = f.failed(ip(&format!("2001:db8:5:5::{i:x}")));
    }
    assert!(f.blocked(ip("2001:db8:5:5::abcd")).is_some());
    assert!(f.blocked(ip("2001:db8:5:6::1")).is_none());
    assert!(!f.is_empty());
    clock.advance(Duration::from_secs(70));
    f.sweep();
    assert!(f.is_empty());
}
