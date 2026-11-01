use std::time::Duration;

use http::{HeaderValue, Request, Response, header};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use tracing::debug;

use super::Body;
use super::forwarded::Client;
use super::pages::Page;
use super::site::{Hop, SiteRun};
use super::unread::Kept;
use super::upstream::{Failed, handshake, refusal_page, strip_hop_by_hop};
use crate::auth::forward::{Asked, copy_granted, strip_visitor_copies, subrequest};
use crate::auth::{Access, Verdict, basic, retry_after};
use crate::route::Table;
use crate::{ForwardAuth, Upstream, Visitor};

pub(crate) struct Gate<'a, U> {
    pub up: &'a U,
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

    pub(crate) async fn check(&self, req: &mut Request<Kept>) -> Option<Response<Body>> {
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
        req: &mut Request<Kept>,
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
        let asked = tokio::time::timeout(wait, self.ask(fa, sub)).await;
        let resp = match asked {
            Ok(Ok(r)) => r,
            Ok(Err(Failed::Refused(why))) => return page(refusal_page(why)),
            Ok(Err(Failed::Broken)) => return page(Page::BadGateway),
            Err(_) => return page(Page::Timeout),
        };
        if resp.status().is_success() {
            copy_granted(resp.headers(), req.headers_mut(), &fa.copy_headers);
            return None;
        }
        let mut resp = resp.map(|b| b.map_err(Into::into).boxed());
        strip_hop_by_hop(resp.headers_mut());
        Some(resp)
    }

    async fn ask(
        &self,
        fa: &ForwardAuth,
        sub: Request<Body>,
    ) -> Result<Response<Incoming>, Failed> {
        let io = self
            .up
            .open_auth(&fa.service, self.visitor)
            .await
            .map_err(Failed::Refused)?;
        let how = self
            .table
            .site(&fa.service)
            .map_or(Hop::Plain, |s| s.hop.clone());
        let (mut tx, _) = handshake(io, &how).await?;
        tx.send_request(sub).await.map_err(|e| {
            debug!(peer = %self.visitor.addr, "web: the login service \"{}\" did not answer: {e}", fa.service);
            Failed::Broken
        })
    }
}
