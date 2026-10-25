use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use bytes::Bytes;
use http::HeaderMap;
use http::header;
use hyper::body::{Body as HttpBody, Frame, SizeHint};

use super::{Body, BoxError};

pub fn declared_over(h: &HeaderMap, max: u64) -> bool {
    h.get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .is_some_and(|n| n > max)
}

#[derive(Debug, Default, Clone)]
pub struct Overflow(Arc<AtomicBool>);

impl Overflow {
    pub fn hit(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Debug)]
pub struct TooLarge;

impl std::fmt::Display for TooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the request body is larger than max_body_size")
    }
}

impl std::error::Error for TooLarge {}

pub struct Capped {
    inner: Body,
    left: u64,
    overflow: Overflow,
}

impl Capped {
    pub fn new(inner: Body, max: u64) -> (Capped, Overflow) {
        let overflow = Overflow::default();
        let capped = Capped {
            inner,
            left: max,
            overflow: overflow.clone(),
        };
        (capped, overflow)
    }
}

impl HttpBody for Capped {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let this = &mut *self;
        match Pin::new(&mut this.inner).poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                let len = frame
                    .data_ref()
                    .map_or(0, |d| u64::try_from(d.len()).unwrap_or(u64::MAX));
                if len > this.left {
                    this.overflow.0.store(true, Ordering::Relaxed);
                    return Poll::Ready(Some(Err(Box::new(TooLarge))));
                }
                this.left -= len;
                Poll::Ready(Some(Ok(frame)))
            }
            other => other,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::full;
    use http_body_util::BodyExt;

    #[test]
    fn a_declared_length_over_the_limit_is_known_before_the_body() {
        let mut h = HeaderMap::new();
        assert!(!declared_over(&h, 10));
        h.insert(header::CONTENT_LENGTH, "10".parse().unwrap());
        assert!(!declared_over(&h, 10));
        h.insert(header::CONTENT_LENGTH, "11".parse().unwrap());
        assert!(declared_over(&h, 10));
    }

    #[tokio::test]
    async fn a_body_that_grows_past_the_limit_ends_with_an_error_and_marks_it() {
        let (ok, flag) = Capped::new(full("0123456789"), 10);
        assert_eq!(ok.collect().await.unwrap().to_bytes().len(), 10);
        assert!(!flag.hit());
        let (big, flag) = Capped::new(full("0123456789x"), 10);
        let e = big.collect().await.err().unwrap();
        assert!(e.is::<TooLarge>());
        assert!(flag.hit());
    }
}
