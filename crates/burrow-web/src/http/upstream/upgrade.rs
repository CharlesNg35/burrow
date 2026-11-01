use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use http::{Request, Response, StatusCode};
use http_body_util::BodyExt;
use hyper::body::{Body as HttpBody, Frame, SizeHint};
use hyper_util::rt::TokioIo;
use tracing::debug;

use super::pool::{H1, Slot, Ups};
use super::send::opened;
use super::{Failed, Forward, handshake, prepare, strip_hop_by_hop};
use crate::http::idle::Activity;
use crate::http::pages::Page;
use crate::http::{Body, BoxError};
use crate::{Ending, Upstream};

pub(super) async fn forward<U: Upstream>(
    up: &U,
    ups: &Ups,
    mut req: Request<Body>,
    f: Forward<'_>,
) -> Response<Body> {
    let downstream = hyper::upgrade::on(&mut req);
    prepare(&mut req, &f);
    let service = f.site.service.as_str();
    let deadline = tokio::time::Instant::now() + f.site.options.response_header_timeout;
    let Some(slot) = ups.slot(deadline).await else {
        return f.page(Page::Timeout);
    };
    let open = connect(up, service, &f);
    let mut conn = match opened(tokio::time::timeout_at(deadline, open).await) {
        Ok(conn) => conn,
        Err(page) => return f.page(page),
    };
    let mut resp = match tokio::time::timeout_at(deadline, conn.tx.send_request(req)).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            debug!(peer = %f.visitor.addr, "web: service \"{service}\" did not answer: {e}");
            let why = conn.ending.as_ref().and_then(Ending::refused);
            return f.page(why.map_or(Page::BadGateway, super::refusal_page));
        }
        Err(_) => return f.page(Page::Timeout),
    };
    if resp.status() == StatusCode::SWITCHING_PROTOCOLS {
        let up = hyper::upgrade::on(&mut resp);
        tokio::spawn(join(downstream, up, f.activity.clone()));
        return resp.map(|b| b.map_err(Into::into).boxed());
    }
    strip_hop_by_hop(resp.headers_mut());
    let activity = f.activity.clone();
    resp.map(move |b| {
        let b = b
            .map_err(move |e| {
                activity.fail();
                BoxError::from(e)
            })
            .boxed();
        Slotted {
            inner: b,
            _slot: slot,
        }
        .boxed()
    })
}

async fn connect<U: Upstream>(up: &U, service: &str, f: &Forward<'_>) -> Result<H1, Failed> {
    let io = up
        .open(service, &f.visitor)
        .await
        .map_err(Failed::Refused)?;
    handshake(io, &f.site.hop).await
}

async fn join(
    down: hyper::upgrade::OnUpgrade,
    up: hyper::upgrade::OnUpgrade,
    activity: Arc<Activity>,
) {
    let (Ok(down), Ok(up)) = (down.await, up.await) else {
        return;
    };
    let (mut down, mut up) = (TokioIo::new(down), TokioIo::new(up));
    if let Err(e) = tokio::io::copy_bidirectional(&mut down, &mut up).await {
        debug!("web: an upgraded connection ended: {e}");
        activity.fail();
    }
}

struct Slotted {
    inner: Body,
    _slot: Slot,
}

impl HttpBody for Slotted {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        Pin::new(&mut self.inner).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}
