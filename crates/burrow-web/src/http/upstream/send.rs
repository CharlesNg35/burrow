use std::sync::Arc;
use std::sync::atomic::Ordering;

use http::{Method, Request, Response};
use http_body_util::{BodyExt, Empty};
use hyper::body::{Body as HttpBody, Incoming};
use tokio::time::Instant;
use tracing::debug;

use super::pool::{Cut, Cutter, EVICT_TRIES, EVICT_WAIT, H1, H2, Home, Key, Pool, Ticket, Ups};
use super::{Failed, handshake, refusal_page, to_h2};
use crate::http::pages::Page;
use crate::http::site::Hop;
use crate::http::{Body, BoxError};
use crate::{Ask, BoxIo, Ending, Lease, Refused, Upstream, Visitor, Way};

pub(crate) struct Asking<'a, U> {
    pub up: &'a U,
    pub pool: &'a Arc<Pool>,
    pub service: &'a Arc<str>,
    pub from: &'a Visitor,
    pub hop: &'a Hop,
    pub way: Way,
    pub lease: Lease,
    pub key: Key,
    pub deadline: Instant,
    pub epoch: u64,
    pub ask: Ask,
}

pub(crate) type Sent = (Response<Incoming>, Home, Ticket);

pub(crate) struct Miss {
    pub page: Page,
    pub reset: Option<h2::Reason>,
}

impl From<Page> for Miss {
    fn from(page: Page) -> Miss {
        Miss { page, reset: None }
    }
}

pub(super) fn opened<T>(
    r: Result<Result<T, Failed>, tokio::time::error::Elapsed>,
) -> Result<T, Page> {
    match r {
        Ok(Ok(t)) => Ok(t),
        Ok(Err(Failed::Refused(why))) => Err(refusal_page(why)),
        Ok(Err(Failed::Broken)) => Err(Page::BadGateway),
        Err(_) => Err(Page::Timeout),
    }
}

struct Head {
    method: Method,
    uri: http::Uri,
    version: http::Version,
    headers: http::HeaderMap,
}

impl Head {
    fn request(self) -> Request<Body> {
        let mut req = Request::new(Empty::new().map_err(|e| -> BoxError { match e {} }).boxed());
        *req.method_mut() = self.method;
        *req.uri_mut() = self.uri;
        *req.version_mut() = self.version;
        *req.headers_mut() = self.headers;
        req
    }
}

fn replayable(req: &Request<Body>) -> Option<Head> {
    let safe = matches!(
        *req.method(),
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    );
    if !safe || !req.body().is_end_stream() {
        return None;
    }
    Some(Head {
        method: req.method().clone(),
        uri: req.uri().clone(),
        version: req.version(),
        headers: req.headers().clone(),
    })
}

fn stale(e: &hyper::Error) -> bool {
    use std::io::ErrorKind as K;
    if e.is_incomplete_message() {
        return true;
    }
    let mut at: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = at {
        if let Some(io) = err.downcast_ref::<std::io::Error>() {
            return matches!(
                io.kind(),
                K::ConnectionReset | K::BrokenPipe | K::ConnectionAborted | K::UnexpectedEof
            );
        }
        at = err.source();
    }
    false
}

fn refused(e: &hyper::Error) -> bool {
    to_h2::app_reset(e) == Some(h2::Reason::REFUSED_STREAM)
}

fn missed(service: &str, from: &Visitor, ending: Option<&Ending>, e: &hyper::Error) -> Miss {
    debug!(peer = %from.addr, "web: service \"{service}\" did not answer: {e}");
    let why = ending.and_then(Ending::refused);
    Miss {
        page: why.map_or(Page::BadGateway, refusal_page),
        reset: to_h2::app_reset(e),
    }
}

impl<U: Upstream> Asking<'_, U> {
    async fn open_io(&self) -> Result<BoxIo, Refused> {
        let wait = self.deadline.min(Instant::now() + EVICT_WAIT);
        let mut tries = 0;
        loop {
            let opened = self
                .up
                .open_pooled(self.service, self.from, &self.lease)
                .await;
            tries += 1;
            if opened.as_ref().err() != Some(&Refused::Full) || tries > EVICT_TRIES {
                return opened;
            }
            let ended = self.pool.evict(self.service, &self.way.client);
            if ended.is_empty() && tries > 1 {
                return opened;
            }
            let all = all_ended(ended);
            if tokio::time::timeout_at(wait, all).await.is_err() {
                return opened;
            }
        }
    }

    async fn connect_h1(&self) -> Result<H1, Failed> {
        let io = self.open_io().await.map_err(Failed::Refused)?;
        handshake(io, self.hop).await
    }

    async fn connect_h2(&self) -> Result<H2, Failed> {
        let io = self.open_io().await.map_err(Failed::Refused)?;
        to_h2::handshake(io, self.hop).await
    }

    async fn open_h1(&self) -> Result<H1, Page> {
        opened(tokio::time::timeout_at(self.deadline, self.connect_h1()).await)
    }

    async fn open_h2(&self) -> Result<H2, Page> {
        opened(tokio::time::timeout_at(self.deadline, self.connect_h2()).await)
    }

    async fn take_h1(&self, ups: Option<&Ups>) -> Option<(H1, Key, Arc<str>, Instant)> {
        if let Some(ups) = ups.filter(|u| u.has_private()) {
            let private = self.key.private(ups.id, self.from.addr.ip());
            while let Some((conn, client, since)) = self.pool.h1.take(&private) {
                if let Some(conn) = conn.ready().await {
                    return Some((conn, private, client, since));
                }
            }
        }
        while let Some((conn, client, since)) = self.pool.h1.take(&self.key) {
            if let Some(conn) = conn.ready().await {
                return Some((conn, self.key.clone(), client, since));
            }
        }
        None
    }

    pub(crate) async fn send_h1(
        &self,
        ups: Option<&Ups>,
        req: Request<Body>,
    ) -> Result<Sent, Miss> {
        let (mut conn, mut key, mut client, reused) = match self.take_h1(ups).await {
            Some((conn, key, client, _)) => (conn, key, client, true),
            None => (
                self.open_h1().await?,
                self.key.clone(),
                self.way.client.clone(),
                false,
            ),
        };
        let copy = if reused { replayable(&req) } else { None };
        let mut ticket = self.listed(Cutter::Connection(conn.task.clone()));
        let read_before = conn.read.load(Ordering::Acquire);
        let sent = tokio::time::timeout_at(self.deadline, conn.tx.try_send_request(req)).await;
        let resp = match sent {
            Err(_) => return Err(Page::Timeout.into()),
            Ok(Ok(r)) => r,
            Ok(Err(mut e)) => {
                let again = e.take_message().or_else(|| {
                    let silent = conn.read.load(Ordering::Acquire) == read_before;
                    copy.filter(|_| silent && stale(e.error()))
                        .map(Head::request)
                });
                let Some(req) = again else {
                    return Err(missed(
                        self.service,
                        self.from,
                        conn.ending.as_ref(),
                        e.error(),
                    ));
                };
                debug!(peer = %self.from.addr, "web: service \"{}\": a kept app connection was closed, sending again on a new one", self.service);
                conn = self.open_h1().await?;
                ticket = self.listed(Cutter::Connection(conn.task.clone()));
                (key, client) = (self.key.clone(), self.way.client.clone());
                let sent = tokio::time::timeout_at(self.deadline, conn.tx.send_request(req));
                match sent.await {
                    Err(_) => return Err(Page::Timeout.into()),
                    Ok(Ok(r)) => r,
                    Ok(Err(e)) => {
                        return Err(missed(self.service, self.from, conn.ending.as_ref(), &e));
                    }
                }
            }
        };
        let key = key.answered(conn.ending.as_ref().and_then(Ending::via));
        let home = Home::H1 {
            pool: self.pool.clone(),
            key,
            client,
            conn,
            epoch: self.epoch,
        };
        Ok((resp, home, ticket))
    }

    fn listed(&self, cutter: Cutter) -> Ticket {
        let mut ticket = self
            .pool
            .ticket(self.service, self.from.clone(), self.ask, cutter);
        ticket.waiting();
        ticket
    }

    pub(crate) async fn send_h2(&self, ups: &Ups, req: Request<Body>) -> Result<Sent, Miss> {
        let (conn, stream, reused) = match ups.take_h2(self.pool, &self.key) {
            Some((conn, stream, _)) => (conn, stream, true),
            None => {
                let conn = self.open_h2().await?;
                let client = self.way.client.clone();
                let stream = ups.reserve(self.pool, (self.key.clone(), client), &conn, self.epoch);
                (conn, stream, false)
            }
        };
        let copy = if reused { replayable(&req) } else { None };
        let cut = Arc::new(Cut::default());
        let ticket = self.listed(Cutter::Stream(cut.clone()));
        let first = self.try_h2(&conn, req, &cut).await;
        let again = match first {
            Ok(resp) => return Ok((resp, Home::H2(stream), ticket)),
            Err(Retry::Timeout) => return Err(Page::Timeout.into()),
            Err(Retry::Cut) => return Err(Page::BadGateway.into()),
            Err(Retry::Unsent(req)) => *req,
            Err(Retry::Failed(e)) => match copy.filter(|_| refused(&e)) {
                Some(head) => head.request(),
                None => return Err(missed(self.service, self.from, conn.ending.as_ref(), &e)),
            },
        };
        drop((stream, ticket));
        debug!(peer = %self.from.addr, "web: service \"{}\": a kept HTTP/2 app connection was closed, sending again on a new one", self.service);
        let conn = self.open_h2().await?;
        let client = self.way.client.clone();
        let stream = ups.reserve(self.pool, (self.key.clone(), client), &conn, self.epoch);
        let cut = Arc::new(Cut::default());
        let ticket = self.listed(Cutter::Stream(cut.clone()));
        match self.try_h2(&conn, again, &cut).await {
            Ok(resp) => Ok((resp, Home::H2(stream), ticket)),
            Err(Retry::Timeout) => Err(Page::Timeout.into()),
            Err(Retry::Cut) => Err(Page::BadGateway.into()),
            Err(Retry::Unsent(_)) => Err(Page::BadGateway.into()),
            Err(Retry::Failed(e)) => Err(missed(self.service, self.from, conn.ending.as_ref(), &e)),
        }
    }

    async fn try_h2(
        &self,
        conn: &H2,
        req: Request<Body>,
        cut: &Cut,
    ) -> Result<Response<Incoming>, Retry> {
        tokio::select! {
            sent = self.send_on(conn, req) => sent,
            () = cut.wait() => Err(Retry::Cut),
        }
    }

    async fn send_on(&self, conn: &H2, req: Request<Body>) -> Result<Response<Incoming>, Retry> {
        let mut tx = conn.tx.clone();
        match tokio::time::timeout_at(self.deadline, tx.ready()).await {
            Err(_) => return Err(Retry::Timeout),
            Ok(Err(_)) => return Err(Retry::Unsent(Box::new(req))),
            Ok(Ok(())) => {}
        }
        match tokio::time::timeout_at(self.deadline, tx.try_send_request(req)).await {
            Err(_) => Err(Retry::Timeout),
            Ok(Ok(r)) => Ok(r),
            Ok(Err(mut e)) => match e.take_message() {
                Some(req) => Err(Retry::Unsent(Box::new(req))),
                None => Err(Retry::Failed(e.into_error())),
            },
        }
    }
}

enum Retry {
    Timeout,
    Cut,
    Unsent(Box<Request<Body>>),
    Failed(hyper::Error),
}

async fn all_ended(ended: Vec<Ending>) {
    for e in ended {
        e.ended().await;
    }
}
