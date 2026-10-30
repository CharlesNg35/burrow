use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, ready};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::Sleep;

const MIN_BURST: f64 = 16.0 * 1024.0;
const MIN_CHUNK: f64 = 1400.0;

pub use burrow_syntax::rate::{describe, parse_rate};

fn burst_of(rate: f64) -> f64 {
    (rate / 10.0).max(MIN_BURST)
}

#[derive(Debug)]
pub struct Bucket {
    rate: AtomicU64,
    state: Mutex<(f64, Instant)>,
}

impl Bucket {
    pub fn new(bytes_per_sec: u64) -> Bucket {
        Bucket {
            rate: AtomicU64::new(bytes_per_sec),
            state: Mutex::new((burst_of(bytes_per_sec as f64), Instant::now())),
        }
    }

    pub fn rate(&self) -> u64 {
        self.rate.load(Ordering::Relaxed)
    }

    pub fn set_rate(&self, bytes_per_sec: u64) {
        let mut state = self.state.lock().expect("bucket lock");
        let old = self.rate.load(Ordering::Relaxed);
        if old == bytes_per_sec {
            return;
        }
        if old == 0 {
            *state = (burst_of(bytes_per_sec as f64), Instant::now());
        } else {
            refill(old as f64, &mut state);
            state.0 = state.0.min(burst_of(bytes_per_sec as f64));
        }
        self.rate.store(bytes_per_sec, Ordering::Relaxed);
    }

    pub fn ready(&self, want: usize) -> Result<usize, Duration> {
        let rate = self.rate();
        if rate == 0 || want == 0 {
            return Ok(want);
        }
        let rate = rate as f64;
        let mut state = self.state.lock().expect("bucket lock");
        refill(rate, &mut state);
        let chunk = (want as f64)
            .min((rate / 50.0).max(MIN_CHUNK))
            .min(burst_of(rate));
        if state.0 >= chunk {
            Ok((state.0 as usize).clamp(1, want))
        } else {
            Err(Duration::from_secs_f64((chunk - state.0) / rate).max(Duration::from_millis(1)))
        }
    }

    pub fn consume(&self, n: usize) {
        let rate = self.rate();
        if rate == 0 {
            return;
        }
        let mut state = self.state.lock().expect("bucket lock");
        refill(rate as f64, &mut state);
        state.0 -= n as f64;
    }

    pub fn try_take(&self, n: usize) -> bool {
        let rate = self.rate();
        if rate == 0 {
            return true;
        }
        let rate = rate as f64;
        let mut state = self.state.lock().expect("bucket lock");
        refill(rate, &mut state);
        if state.0 >= (n as f64).min(burst_of(rate)) {
            state.0 -= n as f64;
            true
        } else {
            false
        }
    }
}

fn refill(rate: f64, state: &mut (f64, Instant)) {
    let now = Instant::now();
    let earned = now.duration_since(state.1).as_secs_f64() * rate;
    state.0 = (state.0 + earned).min(burst_of(rate));
    state.1 = now;
}

#[derive(Debug)]
pub struct Limit {
    pub inbound: Bucket,
    pub outbound: Bucket,
}

impl Limit {
    pub fn new(bytes_per_sec: u64) -> Arc<Limit> {
        Arc::new(Limit {
            inbound: Bucket::new(bytes_per_sec),
            outbound: Bucket::new(bytes_per_sec),
        })
    }

    pub fn rate(&self) -> u64 {
        self.inbound.rate()
    }

    pub fn set_rate(&self, bytes_per_sec: u64) {
        self.inbound.set_rate(bytes_per_sec);
        self.outbound.set_rate(bytes_per_sec);
    }
}

pub struct Throttled<'a, S> {
    inner: S,
    bucket: Option<&'a Bucket>,
    sleep: Option<Pin<Box<Sleep>>>,
    waiting: bool,
}

impl<'a, S> Throttled<'a, S> {
    pub fn new(inner: S, bucket: Option<&'a Bucket>) -> Self {
        Throttled {
            inner,
            bucket,
            sleep: None,
            waiting: false,
        }
    }

    fn allowance(&mut self, cx: &mut Context<'_>, want: usize) -> Poll<usize> {
        let Some(bucket) = self.bucket else {
            return Poll::Ready(want);
        };
        loop {
            if self.waiting {
                if let Some(sleep) = self.sleep.as_mut() {
                    ready!(sleep.as_mut().poll(cx));
                }
                self.waiting = false;
            }
            match bucket.ready(want) {
                Ok(n) => return Poll::Ready(n),
                Err(wait) => {
                    let at = tokio::time::Instant::now() + wait;
                    match self.sleep.as_mut() {
                        Some(sleep) => sleep.as_mut().reset(at),
                        None => self.sleep = Some(Box::pin(tokio::time::sleep_until(at))),
                    }
                    self.waiting = true;
                }
            }
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Throttled<'_, S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        let allowed = ready!(me.allowance(cx, buf.remaining()));
        if allowed >= buf.remaining() {
            let before = buf.filled().len();
            ready!(Pin::new(&mut me.inner).poll_read(cx, buf))?;
            if let Some(b) = me.bucket {
                b.consume(buf.filled().len() - before);
            }
            return Poll::Ready(Ok(()));
        }
        let mut part = ReadBuf::new(buf.initialize_unfilled_to(allowed));
        ready!(Pin::new(&mut me.inner).poll_read(cx, &mut part))?;
        let n = part.filled().len();
        buf.advance(n);
        if let Some(b) = me.bucket {
            b.consume(n);
        }
        Poll::Ready(Ok(()))
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Throttled<'_, S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let me = self.get_mut();
        let allowed = ready!(me.allowance(cx, data.len()));
        let n = ready!(Pin::new(&mut me.inner).poll_write(cx, &data[..allowed]))?;
        if let Some(b) = me.bucket {
            b.consume(n);
        }
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn rates_parse() {
        assert_eq!(parse_rate("40mbit").unwrap(), 5_000_000);
        assert_eq!(parse_rate("40Mbit/s").unwrap(), 5_000_000);
        assert_eq!(parse_rate("1gbit").unwrap(), 125_000_000);
        assert_eq!(parse_rate("500kbit").unwrap(), 62_500);
        assert_eq!(parse_rate("5MB").unwrap(), 5_000_000);
        assert_eq!(parse_rate("2.5mbps").unwrap(), 312_500);
        assert_eq!(parse_rate("100kb").unwrap(), 100_000);
        assert!(parse_rate("fast").is_err());
        assert!(
            parse_rate("10 apples")
                .unwrap_err()
                .contains("not a rate unit")
        );
        assert!(parse_rate("0mbit").is_err());
        assert!(parse_rate("1kbit").unwrap_err().contains("minimum"));
        assert_eq!(describe(5_000_000), "40mbit");
        assert_eq!(describe(62_500), "500kbit");
        assert_eq!(describe(125_000_000), "1gbit");
        assert_eq!(describe(312_500), "2.5mbit");
    }

    #[test]
    fn bytes_and_bits_per_second_are_told_apart() {
        assert_eq!(parse_rate("5MBps").unwrap(), 5_000_000);
        assert_eq!(parse_rate("5Mbps").unwrap(), 625_000);
        assert_eq!(parse_rate("1kb").unwrap(), 1000);
        assert_eq!(parse_rate("8kbit").unwrap(), 1000);
        assert!(parse_rate("999b").is_err());
    }

    #[test]
    fn a_rate_changes_in_place() {
        let b = Bucket::new(0);
        assert_eq!(b.ready(1_000_000), Ok(1_000_000), "0 means unlimited");
        assert!(b.try_take(1_000_000));
        b.set_rate(100_000);
        assert!(b.try_take(16_000));
        assert!(!b.try_take(16_000), "the new limit applies at once");
        b.set_rate(0);
        assert!(b.try_take(1_000_000), "removing the limit lifts it");
    }

    #[test]
    fn a_datagram_larger_than_the_burst_passes_an_idle_bucket() {
        let b = Bucket::new(100_000);
        assert!(b.try_take(60_000));
        assert!(!b.try_take(1), "the bucket is in debt now");
    }

    #[test]
    fn datagrams_over_the_rate_are_refused() {
        let b = Bucket::new(100_000);
        assert!(b.try_take(16_000));
        assert!(!b.try_take(16_000), "the burst is spent");
    }

    #[tokio::test]
    async fn reads_and_writes_follow_the_rate() {
        let limit = Limit::new(400_000);
        let (a, mut b) = tokio::io::duplex(1 << 20);
        let data = vec![7u8; 200_000];
        let started = Instant::now();
        let writer = {
            let data = data.clone();
            let limit = limit.clone();
            tokio::spawn(async move {
                let mut w = Throttled::new(a, Some(&limit.outbound));
                w.write_all(&data).await.unwrap();
                w.shutdown().await.unwrap();
            })
        };
        let mut got = Vec::new();
        b.read_to_end(&mut got).await.unwrap();
        writer.await.unwrap();
        let took = started.elapsed();
        assert_eq!(got, data);
        assert!(took >= Duration::from_millis(350), "too fast: {took:?}");
        assert!(took <= Duration::from_millis(1500), "too slow: {took:?}");

        let (mut a, b) = tokio::io::duplex(1 << 20);
        a.write_all(&data).await.unwrap();
        drop(a);
        let started = Instant::now();
        let mut r = Throttled::new(b, Some(&limit.inbound));
        let mut got = Vec::new();
        r.read_to_end(&mut got).await.unwrap();
        let took = started.elapsed();
        assert_eq!(got.len(), data.len());
        assert!(took >= Duration::from_millis(350), "too fast: {took:?}");
    }

    #[tokio::test]
    async fn a_throttled_stream_reuses_one_timer_for_every_wait() {
        let limit = Limit::new(400_000);
        let (a, mut b) = tokio::io::duplex(1 << 20);
        let reader = tokio::spawn(async move {
            let mut got = Vec::new();
            b.read_to_end(&mut got).await.unwrap();
            got.len()
        });
        let mut w = Throttled::new(a, Some(&limit.outbound));
        let data = vec![7u8; 100_000];
        let timer = |w: &Throttled<'_, _>| w.sleep.as_ref().map(|s| &**s as *const Sleep);
        w.write_all(&data).await.unwrap();
        let first = timer(&w);
        assert!(first.is_some(), "the rate made the writer wait");
        w.write_all(&data).await.unwrap();
        assert_eq!(timer(&w), first);
        w.shutdown().await.unwrap();
        drop(w);
        assert_eq!(reader.await.unwrap(), 200_000);
    }

    #[tokio::test]
    async fn no_bucket_means_no_limit() {
        let (mut a, b) = tokio::io::duplex(1 << 20);
        a.write_all(&[1u8; 500_000]).await.unwrap();
        drop(a);
        let started = Instant::now();
        let mut got = Vec::new();
        Throttled::new(b, None).read_to_end(&mut got).await.unwrap();
        assert_eq!(got.len(), 500_000);
        assert!(started.elapsed() < Duration::from_millis(300));
    }
}
