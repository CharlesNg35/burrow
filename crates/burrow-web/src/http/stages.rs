use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use http::{HeaderMap, HeaderValue, Method, Request, Response, Version};
use http_body_util::BodyExt;
use hyper::body::Incoming;

use super::access::Gate;
use super::forwarded::{self, Client, REQUEST_ID};
use super::limit::{Capped, declared_over};
use super::logged::Logged;
use super::pages::Page;
use super::site::SiteRun;
use super::upstream::{Forward, forward};
use super::{Body, Conn, compress, headers, redirect};
use crate::listen::Shared;
use crate::log::{Entry, kept_request_id, new_request_id};
use crate::route::{Route, Table};
use crate::{Upstream, Visitor};

pub(crate) struct Asked {
    pub host: String,
    pub route: Route,
    pub service: String,
}

fn request_id(h: &HeaderMap, client: &Client) -> String {
    h.get(REQUEST_ID)
        .and_then(|v| v.to_str().ok())
        .filter(|v| client.trusted_peer && kept_request_id(v))
        .map_or_else(new_request_id, str::to_string)
}

fn protocol(v: Version) -> &'static str {
    match v {
        Version::HTTP_10 => "HTTP/1.0",
        Version::HTTP_2 => "HTTP/2.0",
        _ => "HTTP/1.1",
    }
}

fn entry(req: &Request<Incoming>, client: &Client, asked: &Asked, id: Option<&str>) -> Entry {
    Entry {
        time: SystemTime::now(),
        client: client.ip,
        host: asked.host.clone(),
        method: req.method().to_string(),
        target: req
            .uri()
            .path_and_query()
            .map_or("/", |p| p.as_str())
            .to_string(),
        protocol: protocol(req.version()),
        status: 0,
        bytes: 0,
        ms: 0,
        service: asked.service.clone(),
        request_id: id.map(str::to_string),
    }
}

fn app_target(req: &mut Request<Body>, route: &Route) -> Option<String> {
    let app_path = route.app_path.as_deref()?;
    let path = redirect::app_path(&route.prefix, app_path, req.uri().path());
    if path == req.uri().path() {
        return None;
    }
    let target = match req.uri().query() {
        Some(q) => format!("{path}?{q}"),
        None => path,
    };
    *req.uri_mut() = target.parse().ok()?;
    Some(route.prefix.clone())
}

pub(crate) async fn to_service<U: Upstream>(
    shared: &Arc<Shared<U>>,
    conn: &Conn,
    table: &Table,
    asked: Asked,
    req: Request<Incoming>,
) -> Response<Body> {
    let started = Instant::now();
    let Some(site) = table.site(&asked.service).cloned() else {
        return Page::NotFound.response();
    };
    let https = conn.name.is_some();
    let client = forwarded::client(
        conn.peer,
        req.headers(),
        &table.forwarding.trusted_proxies,
        https,
    );
    let id = table.request_id.then(|| request_id(req.headers(), &client));
    let log = shared.access_log();
    let logged = log
        .as_ref()
        .map(|_| entry(&req, &client, &asked, id.as_deref()));
    let method = req.method().clone();
    let gzip = compress::accepts_gzip(req.headers());
    let mut resp = answer(
        shared,
        conn,
        (table, &site),
        &asked,
        client,
        id.as_deref(),
        req,
    )
    .await;
    finish(&site, &method, https, gzip, id.as_deref(), &mut resp);
    shared.up.answered(&asked.service, resp.status());
    if let (Some((sink, format)), Some(mut e)) = (log, logged) {
        e.status = resp.status().as_u16();
        resp = resp.map(|b| Logged::new(b, e, started, sink, format).boxed());
    }
    resp
}

fn finish(
    site: &SiteRun,
    method: &Method,
    https: bool,
    gzip: bool,
    id: Option<&str>,
    resp: &mut Response<Body>,
) {
    let o = &site.options;
    let h = resp.headers_mut();
    headers::apply(&o.response_headers, h);
    if o.hsts {
        headers::add_hsts(h, https);
    }
    if let Some(v) = id.and_then(|i| HeaderValue::from_str(i).ok()) {
        h.insert(REQUEST_ID, v);
    }
    if o.compress {
        let taken = std::mem::replace(resp, Response::new(super::full("")));
        *resp = compress::respond(method, gzip, taken);
    }
}

async fn answer<U: Upstream>(
    shared: &Arc<Shared<U>>,
    conn: &Conn,
    (table, site): (&Table, &SiteRun),
    asked: &Asked,
    client: Client,
    id: Option<&str>,
    mut req: Request<Incoming>,
) -> Response<Body> {
    let o = &site.options;
    let visitor = Visitor {
        addr: SocketAddr::new(client.ip, conn.peer.port()),
        local: conn.local,
        host: asked.host.clone(),
    };
    let gate = Gate {
        up: shared.up.as_ref(),
        access: &shared.access,
        table,
        site,
        visitor: &visitor,
        client: &client,
    };
    if let Some(denied) = gate.allowed() {
        return denied;
    }
    if o.maintenance {
        return Page::Maintenance.response_from(&site.pages);
    }
    if let Some(refused) = gate.check(&mut req).await {
        return refused;
    }
    let mut overflow = None;
    let mut req = match o.max_body_size {
        Some(max) if declared_over(req.headers(), max) => {
            return Page::TooLarge.response_from(&site.pages);
        }
        Some(max) => req.map(|b| {
            let (capped, flag) = Capped::new(b.map_err(Into::into).boxed(), max);
            overflow = Some(flag);
            capped.boxed()
        }),
        None => req.map(|b| b.map_err(Into::into).boxed()),
    };
    let prefix = app_target(&mut req, &asked.route);
    let f = Forward {
        site,
        visitor,
        client,
        prefix: prefix.as_deref(),
        request_id: id,
        activity: &conn.activity,
    };
    let resp = forward(shared.up.as_ref(), &conn.ups, req, f).await;
    if overflow.is_some_and(|o| o.hit()) {
        return Page::TooLarge.response_from(&site.pages);
    }
    resp
}
