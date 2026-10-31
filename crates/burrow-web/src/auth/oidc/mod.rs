mod cookies;
mod pages;
mod provider;
mod seal;
mod token;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};

use burrow_syntax::web::oidc::{CALLBACK_PATH, LOGIN_PATH, LOGOUT_PATH, RESERVED, user_allowed};
use http::header::{self, HeaderMap, HeaderName, HeaderValue};
use http::{Request, Response, StatusCode};
use tracing::{debug, info, warn};

use crate::auth::forward::strip_visitor_copies;
use crate::http::host::host_port;
use crate::http::redirect::address;
use crate::http::{Body, full};
use crate::route::{Action, Table};
use crate::{OidcProvider, SiteLogin};

pub use cookies::{PENDING_LIFE, SESSION_LIFE};
pub use seal::KEY_FILE;

use cookies::{Pending, Session};
use provider::Provider;
use seal::Sealer;

pub const REMOTE_USER: HeaderName = HeaderName::from_static("remote-user");
pub const REMOTE_EMAIL: HeaderName = HeaderName::from_static("remote-email");

pub(crate) struct Logins {
    sealer: Sealer,
    providers: RwLock<HashMap<String, Arc<Provider>>>,
}

impl Logins {
    pub(crate) fn new(state_dir: Option<&Path>) -> Logins {
        Logins {
            sealer: Sealer::load(state_dir),
            providers: RwLock::new(HashMap::new()),
        }
    }

    pub(crate) fn apply(&self, list: &[OidcProvider]) {
        let mut held = self.providers.write().expect("providers lock");
        let next = list
            .iter()
            .map(|cfg| {
                let kept = held.get(&cfg.name).filter(|p| p.cfg == *cfg).cloned();
                let p = kept.unwrap_or_else(|| Arc::new(Provider::new(cfg.clone())));
                (cfg.name.clone(), p)
            })
            .collect();
        *held = next;
    }

    fn provider(&self, name: &str) -> Option<Arc<Provider>> {
        self.providers
            .read()
            .expect("providers lock")
            .get(name)
            .cloned()
    }
}

pub(crate) struct Ask<'a> {
    pub logins: &'a Logins,
    pub table: &'a Table,
    pub host: &'a str,
    pub login: &'a SiteLogin,
}

pub fn browser(h: &HeaderMap) -> bool {
    let text = |n: &str| h.get(n).and_then(|v| v.to_str().ok());
    let html = text("accept").is_some_and(|a| a.contains("text/html"));
    let xhr = text("x-requested-with").is_some_and(|v| v.eq_ignore_ascii_case("xmlhttprequest"));
    let mode = text("sec-fetch-mode");
    html && !xhr && mode.is_none_or(|m| m.eq_ignore_ascii_case("navigate"))
}

pub fn local_path(to: &str) -> &str {
    let good = to.starts_with('/') && !to.starts_with("//") && !to.contains('\\');
    if good { to } else { "/" }
}

fn query<B>(req: &Request<B>) -> HashMap<String, String> {
    let q = req.uri().query().unwrap_or("");
    url::form_urlencoded::parse(q.as_bytes())
        .into_owned()
        .collect()
}

fn see_other(location: &str) -> Response<Body> {
    let mut r = Response::new(full(""));
    *r.status_mut() = StatusCode::SEE_OTHER;
    let h = r.headers_mut();
    if let Ok(v) = HeaderValue::from_str(location) {
        h.insert(header::LOCATION, v);
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

fn with_cookies(
    mut r: Response<Body>,
    cookies: impl IntoIterator<Item = HeaderValue>,
) -> Response<Body> {
    for c in cookies {
        r.headers_mut().append(header::SET_COOKIE, c);
    }
    r
}

impl Ask<'_> {
    fn callback_url<B>(&self, req: &Request<B>) -> String {
        let port = host_port(req.uri(), req.headers());
        format!("{}{CALLBACK_PATH}", address(true, self.host, port))
    }

    fn login_of(&self, to: &str) -> Option<SiteLogin> {
        let route = self
            .table
            .route(self.host, to.split('?').next().unwrap_or("/"))?;
        let Action::Service(s) = &route.action else {
            return None;
        };
        self.table.site(s)?.options.oidc.clone()
    }

    pub(crate) async fn check<B>(&self, req: &mut Request<B>) -> Option<Response<Body>> {
        if req.uri().path().starts_with(RESERVED) {
            return Some(self.reserved(req).await);
        }
        strip_visitor_copies(req.headers_mut(), &[&[REMOTE_USER, REMOTE_EMAIL]]);
        let s = &self.logins.sealer;
        let session = cookies::session(s, req.headers(), self.host)
            .filter(|ss| self.login.providers.contains(&ss.p));
        if let Some(ss) = session {
            if !user_allowed(&self.login.allow_users, &ss.e) {
                return Some(pages::not_allowed(&ss.e));
            }
            let h = req.headers_mut();
            cookies::strip_ours(h);
            if let Ok(v) = HeaderValue::from_str(&ss.e) {
                h.insert(REMOTE_USER, v.clone());
                h.insert(REMOTE_EMAIL, v);
            }
            return None;
        }
        if !browser(req.headers()) {
            return Some(pages::login_first());
        }
        let to = req
            .uri()
            .path_and_query()
            .map_or("/", |p| p.as_str())
            .to_string();
        Some(match self.login.providers.as_slice() {
            [one] => self.start(req, one, &to).await,
            many => pages::chooser(many, &to),
        })
    }

    async fn reserved<B>(&self, req: &Request<B>) -> Response<Body> {
        match req.uri().path() {
            LOGIN_PATH => {
                let q = query(req);
                let to = local_path(q.get("to").map_or("/", String::as_str)).to_string();
                let with = q.get("with").map_or("", String::as_str);
                match self.login_of(&to) {
                    Some(l) if l.providers.iter().any(|p| p == with) => {
                        self.start(req, with, &to).await
                    }
                    _ => pages::not_found(),
                }
            }
            CALLBACK_PATH => self.callback(req).await,
            LOGOUT_PATH => {
                let mut names = cookies::names(req.headers());
                names.push(cookies::SESSION.to_string());
                names.sort();
                names.dedup();
                with_cookies(pages::logged_out(), names.iter().map(|n| cookies::clear(n)))
            }
            _ => pages::not_found(),
        }
    }

    async fn start<B>(&self, req: &Request<B>, name: &str, to: &str) -> Response<Body> {
        let Some(p) = self.logins.provider(name) else {
            return pages::not_found();
        };
        match token::start(&p, &self.callback_url(req)).await {
            Ok(st) => {
                let pending = Pending {
                    p: name.to_string(),
                    s: st.state.clone(),
                    v: st.verifier,
                    n: st.nonce,
                    t: to.to_string(),
                    x: cookies::deadline(PENDING_LIFE),
                };
                let n = cookies::pending_name(&st.state);
                let c = cookies::set(&self.logins.sealer, &n, self.host, &pending, PENDING_LIFE);
                with_cookies(see_other(&st.url), [c])
            }
            Err(e) => {
                warn!("web: login provider \"{name}\": {e}");
                pages::failed()
            }
        }
    }

    async fn callback<B>(&self, req: &Request<B>) -> Response<Body> {
        let q = query(req);
        let state = q.get("state").map_or("", String::as_str);
        let s = &self.logins.sealer;
        let found = (!state.is_empty())
            .then(|| cookies::pending(s, req.headers(), self.host, state))
            .flatten();
        let Some(pending) = found else {
            return pages::stale();
        };
        let done = cookies::clear(&cookies::pending_name(state));
        if let Some(e) = q.get("error") {
            debug!(
                "web: login with \"{}\" on {} ended: {e}",
                pending.p, self.host
            );
            return with_cookies(pages::cancelled(e), [done]);
        }
        let answer = self
            .finish(req, &pending, q.get("code").map_or("", String::as_str))
            .await;
        with_cookies(answer, [done])
    }

    async fn finish<B>(&self, req: &Request<B>, pending: &Pending, code: &str) -> Response<Body> {
        let login = self.login_of(&pending.t);
        let p = self.logins.provider(&pending.p);
        let (Some(login), Some(p)) = (login, p) else {
            return pages::stale();
        };
        if !login.providers.contains(&pending.p) || code.is_empty() {
            return pages::stale();
        }
        let id = match token::exchange(&p, &self.callback_url(req), code, &pending.v).await {
            Ok(t) => p.verify(&t, &pending.n).await,
            Err(e) => Err(e),
        };
        let who = match id {
            Ok(who) => who,
            Err(e) => {
                warn!(
                    "web: login with \"{}\" on {} failed: {e}",
                    pending.p, self.host
                );
                return pages::failed();
            }
        };
        if !user_allowed(&login.allow_users, &who.email) {
            info!(
                "web: {} logged in with \"{}\" on {} but is not in allow_users",
                who.email, pending.p, self.host
            );
            return pages::not_allowed(&who.email);
        }
        info!(
            "web: {} logged in with \"{}\" on {}",
            who.email, pending.p, self.host
        );
        let session = Session {
            p: pending.p.clone(),
            e: who.email,
            x: cookies::deadline(SESSION_LIFE),
        };
        let c = cookies::set(
            &self.logins.sealer,
            cookies::SESSION,
            self.host,
            &session,
            SESSION_LIFE,
        );
        with_cookies(see_other(local_path(&pending.t)), [c])
    }
}

#[cfg(test)]
mod tests;
