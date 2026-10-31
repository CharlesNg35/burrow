use std::collections::HashMap;
use std::net::IpAddr;
use std::num::NonZeroU32;
use std::sync::Mutex;
use std::time::Duration;

use governor::clock::{Clock, DefaultClock, Reference};
use governor::middleware::StateInformationMiddleware;
use governor::state::keyed::DefaultKeyedStateStore;
use governor::{NotUntil, Quota, RateLimiter};

use crate::permit::subnet_key;

pub const WRONG_PASSWORDS_PER_MINUTE: NonZeroU32 = NonZeroU32::new(10).expect("ten is not zero");

type Keyed<C> = RateLimiter<IpAddr, DefaultKeyedStateStore<IpAddr>, C, StateInformationMiddleware>;

fn keyed<C: Clock>(quota: Quota, clock: C) -> Keyed<C> {
    RateLimiter::hashmap_with_clock(quota, clock).with_middleware::<StateInformationMiddleware>()
}

fn wait<C: Clock>(limiter: &Keyed<C>, not: &NotUntil<C::Instant>) -> Duration {
    not.wait_time_from(limiter.clock().now())
}

pub fn retry_after(wait: Duration) -> u64 {
    let secs = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
    secs.max(1)
}

pub struct Limit<C: Clock = DefaultClock> {
    limiter: Keyed<C>,
    rate: NonZeroU32,
}

impl Limit {
    pub fn per_second(rate: NonZeroU32) -> Limit {
        Limit::with_clock(rate, DefaultClock::default())
    }
}

impl<C: Clock> Limit<C> {
    pub fn with_clock(rate: NonZeroU32, clock: C) -> Limit<C> {
        let burst = rate.saturating_mul(NonZeroU32::new(2).expect("two is not zero"));
        Limit {
            limiter: keyed(Quota::per_second(rate).allow_burst(burst), clock),
            rate,
        }
    }

    pub fn rate(&self) -> NonZeroU32 {
        self.rate
    }

    pub fn check(&self, ip: IpAddr) -> Result<(), Duration> {
        match self.limiter.check_key(&subnet_key(ip)) {
            Ok(_) => Ok(()),
            Err(not) => Err(wait(&self.limiter, &not)),
        }
    }

    pub fn sweep(&self) {
        self.limiter.retain_recent();
        self.limiter.shrink_to_fit();
    }

    pub fn len(&self) -> usize {
        self.limiter.len()
    }

    pub fn is_empty(&self) -> bool {
        self.limiter.is_empty()
    }
}

pub struct Failures<C: Clock = DefaultClock> {
    limiter: Keyed<C>,
    blocked: Mutex<HashMap<IpAddr, C::Instant>>,
}

impl Default for Failures {
    fn default() -> Failures {
        Failures::with_clock(DefaultClock::default())
    }
}

impl<C: Clock> Failures<C> {
    pub fn with_clock(clock: C) -> Failures<C> {
        Failures {
            limiter: keyed(Quota::per_minute(WRONG_PASSWORDS_PER_MINUTE), clock),
            blocked: Mutex::new(HashMap::new()),
        }
    }

    pub fn blocked(&self, ip: IpAddr) -> Option<Duration> {
        let key = subnet_key(ip);
        let now = self.limiter.clock().now();
        let mut blocked = self.blocked.lock().expect("failures lock");
        let until = *blocked.get(&key)?;
        if until > now {
            return Some(until.duration_since(now).into());
        }
        blocked.remove(&key);
        None
    }

    pub fn failed(&self, ip: IpAddr) -> Option<Duration> {
        let key = subnet_key(ip);
        let not = match self.limiter.check_key(&key) {
            Ok(s) if s.remaining_burst_capacity() > 0 => return None,
            Ok(_) => self.limiter.check_key(&key).err()?,
            Err(not) => not,
        };
        let wait = wait(&self.limiter, &not);
        self.blocked
            .lock()
            .expect("failures lock")
            .insert(key, not.earliest_possible());
        Some(wait)
    }

    pub fn sweep(&self) {
        self.limiter.retain_recent();
        self.limiter.shrink_to_fit();
        let now = self.limiter.clock().now();
        let mut blocked = self.blocked.lock().expect("failures lock");
        blocked.retain(|_, until| *until > now);
        blocked.shrink_to_fit();
    }

    pub fn len(&self) -> usize {
        self.limiter.len() + self.blocked.lock().expect("failures lock").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests;
