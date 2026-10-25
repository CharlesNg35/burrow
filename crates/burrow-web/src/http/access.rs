use std::time::Duration;

use http::{HeaderValue, Request, Response, header};
use hyper::body::Incoming;

use super::Body;
use super::forwarded::Client;
use super::pages::Page;
use super::site::SiteRun;
use crate::auth::{Access, Verdict, basic, retry_after};
use crate::{Upstream, Visitor};

pub(crate) struct Gate<'a, U> {
    pub up: &'a U,
    pub access: &'a Access,
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
        None
    }
}
