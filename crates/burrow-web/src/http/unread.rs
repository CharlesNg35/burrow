use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Buf;
use http::{HeaderValue, Request, Response, StatusCode, Version, header};
use http_body_util::BodyExt;
use hyper::body::{Body as HttpBody, Frame, Incoming, SizeHint};

use super::Body;
use super::headers::has_token;

pub const DISCARD_BYTES: u64 = 256 * 1024;
pub const DISCARD_TIME: Duration = Duration::from_secs(1);

type Slot<B> = Arc<Mutex<Option<B>>>;

pub(crate) struct Kept<B = Incoming> {
    inner: Option<B>,
    back: Option<Slot<B>>,
}

pub(crate) struct Unread<B = Incoming>(Slot<B>);

pub(crate) fn keep<B: HttpBody>(req: Request<B>) -> (Request<Kept<B>>, Option<Unread<B>>) {
    let h = req.headers();
    let wanted = req.version() == Version::HTTP_11
        && !req.body().is_end_stream()
        && !h.contains_key(header::EXPECT)
        && !has_token(h, header::CONNECTION, "close");
    let slot: Option<Slot<B>> = wanted.then(Slot::default);
    let unread = slot.clone().map(Unread);
    let req = req.map(|b| Kept {
        inner: Some(b),
        back: slot,
    });
    (req, unread)
}

impl<B: HttpBody + Unpin> HttpBody for Kept<B> {
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<B::Data>, B::Error>>> {
        let this = self.get_mut();
        this.back = None;
        match this.inner.as_mut() {
            Some(b) => Pin::new(b).poll_frame(cx),
            None => Poll::Ready(None),
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.as_ref().is_none_or(HttpBody::is_end_stream)
    }

    fn size_hint(&self) -> SizeHint {
        self.inner
            .as_ref()
            .map_or_else(|| SizeHint::with_exact(0), HttpBody::size_hint)
    }
}

impl<B> Drop for Kept<B> {
    fn drop(&mut self) {
        if let (Some(back), Some(inner)) = (self.back.take(), self.inner.take())
            && let Ok(mut slot) = back.lock()
        {
            *slot = Some(inner);
        }
    }
}

impl<B> Unread<B>
where
    B: HttpBody + Unpin,
    B::Data: Buf,
{
    pub(crate) async fn settle(self, resp: &mut Response<Body>) {
        let left = self.0.lock().ok().and_then(|mut s| s.take());
        let Some(body) = left else {
            return;
        };
        let h = resp.headers();
        if resp.status() == StatusCode::SWITCHING_PROTOCOLS
            || has_token(h, header::CONNECTION, "close")
        {
            return;
        }
        if drained(body).await {
            return;
        }
        resp.headers_mut()
            .insert(header::CONNECTION, HeaderValue::from_static("close"));
    }
}

async fn drained<B>(body: B) -> bool
where
    B: HttpBody + Unpin,
    B::Data: Buf,
{
    if body.size_hint().exact().is_some_and(|n| n > DISCARD_BYTES) {
        return false;
    }
    tokio::time::timeout(DISCARD_TIME, discard(body))
        .await
        .unwrap_or(false)
}

async fn discard<B>(mut body: B) -> bool
where
    B: HttpBody + Unpin,
    B::Data: Buf,
{
    let mut left = DISCARD_BYTES;
    while let Some(frame) = body.frame().await {
        let Ok(frame) = frame else {
            return false;
        };
        if let Some(d) = frame.data_ref() {
            let n = u64::try_from(d.remaining()).unwrap_or(u64::MAX);
            let Some(rest) = left.checked_sub(n) else {
                return false;
            };
            left = rest;
        }
    }
    true
}

#[cfg(test)]
mod tests;
