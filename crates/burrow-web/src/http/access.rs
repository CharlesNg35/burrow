use std::sync::Arc;
use std::time::Duration;

use http::{HeaderValue, Request, Response, header};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use tokio::time::Instant;

use super::Body;
use super::forwarded::Client;
use super::pages::Page;
use super::site::{Hop, SiteRun};
use super::upstream::pool::{Back, Key, Pool};
use super::upstream::send::Asking;
use super::upstream::{refusal_page, strip_hop_by_hop};
use crate::auth::forward::{Asked, copy_granted, strip_visitor_copies, subrequest};
use crate::auth::{Access, Verdict, basic, retry_after};
use crate::route::Table;
use crate::{Ask, ForwardAuth, Upstream, Visitor};

const AUTH_BODY: usize = 64 * 1024;
const AUTH_DRAIN: Duration = Duration::from_secs(1);

pub(crate) struct Gate<'a, U> {
    pub up: &'a U,
    pub pool: &'a Arc<Pool>,
    pub access: &'a Access,
    pub table: &'a Table,
    pub site: &'a SiteRun,
    pub visitor: &'a Visitor,
    pub client: &'a Client,
}

fn too_many(site: &SiteRun, wait: Duration) -> Response<Body> {
    let mut r = Page::TooMany.response_from(&site.pages);
    r.headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from(retry_after(wait)));
    r
}

fn login(site: &SiteRun) -> Response<Body> {
    let mut r = Page::LoginNeeded.response();
    if let Ok(v) = HeaderValue::from_str(&basic::challenge(&site.service)) {
        r.headers_mut().insert(header::WWW_AUTHENTICATE, v);
    }
    r
}

impl<U: Upstream> Gate<'_, U> {
    pub(crate) fn allowed(&self) -> Option<Response<Body>> {
        let ok = self.up.allowed(&self.site.service, self.visitor);
        (!ok).then(|| Page::Forbidden.response_from(&self.site.pages))
    }

    pub(crate) async fn check(&self, req: &mut Request<Incoming>) -> Option<Response<Body>> {
        if let Some(l) = &self.site.limit
            && let Err(wait) = l.check(self.client.ip)
        {
            return Some(too_many(self.site, wait));
        }
        if !self.site.users.is_empty() {
            let given = req
                .headers()
                .get(header::AUTHORIZATION)
                .map(HeaderValue::as_bytes);
            match self
                .access
                .check(&self.site.users, given, self.client.ip)
                .await
            {
                Verdict::Granted => {
                    req.headers_mut().remove(header::AUTHORIZATION);
                }
                Verdict::Wrong => return Some(login(self.site)),
                Verdict::Wait(wait) => return Some(too_many(self.site, wait)),
                Verdict::Busy => return Some(Page::Busy.response_from(&self.site.pages)),
            }
        }
        match &self.site.options.forward_auth {
            Some(fa) => self.forward_auth(fa, req).await,
            None => None,
        }
    }

    async fn forward_auth(
        &self,
        fa: &ForwardAuth,
        req: &mut Request<Incoming>,
    ) -> Option<Response<Body>> {
        strip_visitor_copies(req.headers_mut(), &[&fa.copy_headers, &fa.strip_headers]);
        let pq = req.uri().path_and_query().map_or("/", |p| p.as_str());
        let asked = Asked {
            method: req.method(),
            host: &self.visitor.host,
            path_and_query: pq,
            client: self.client,
        };
        let page = |p: Page| Some(p.response_from(&self.site.pages));
        let Some(sub) = subrequest(&fa.path, req.headers(), &asked) else {
            return page(Page::BadGateway);
        };
        let wait = self.site.options.response_header_timeout;
        let asked = ask(
            self.up,
            self.pool,
            (self.table, wait),
            (fa, self.visitor),
            sub,
        );
        let asked = tokio::time::timeout(wait, asked).await;
        let mut resp = match asked {
            Ok(Ok(r)) => r,
            Ok(Err(p)) => return page(p),
            Err(_) => return page(Page::Timeout),
        };
        if resp.status().is_success() {
            copy_granted(resp.headers(), req.headers_mut(), &fa.copy_headers);
            let _ = tokio::time::timeout(AUTH_DRAIN, drain(resp.into_body())).await;
            return None;
        }
        strip_hop_by_hop(resp.headers_mut());
        Some(resp)
    }
}

async fn ask<U: Upstream>(
    up: &U,
    pool: &Arc<Pool>,
    (table, wait): (&Table, Duration),
    (fa, visitor): (&ForwardAuth, &Visitor),
    sub: Request<Body>,
) -> Result<Response<Body>, Page> {
    let deadline = Instant::now() + wait;
    let (way, lease) = up
        .lease(&fa.service, visitor, Ask::Auth)
        .map_err(refusal_page)?;
    let site = table.site(&fa.service);
    let hop = site.map_or(Hop::Plain, |s| s.hop.clone());
    let name = site.map_or_else(|| Arc::from(fa.service.as_str()), |s| s.name.clone());
    let key = Key::new(&name, &way, site.map_or(0, |s| s.app_key));
    let asking = Asking {
        up,
        pool,
        service: &name,
        from: visitor,
        hop: &hop,
        way,
        lease,
        key,
        deadline,
        epoch: pool.epoch(),
    };
    let (resp, home) = asking.send_h1(None, sub).await.map_err(|m| m.page)?;
    let ticket = pool.enter(&name, visitor, Ask::Auth, home.task());
    let lease = (asking.lease, ticket);
    Ok(resp.map(|b| Back::new(b.map_err(Into::into).boxed(), home, lease, None).boxed()))
}

async fn drain(mut body: Body) {
    let mut left = AUTH_BODY;
    while let Some(Ok(frame)) = body.frame().await {
        let n = frame.data_ref().map_or(0, bytes::Buf::remaining);
        let Some(rest) = left.checked_sub(n) else {
            return;
        };
        left = rest;
    }
}
