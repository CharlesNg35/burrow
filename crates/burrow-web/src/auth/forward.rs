use http::header::{self, HeaderMap, HeaderName, HeaderValue};
use http::{Method, Request};

use crate::http::forwarded::{
    Client, FORWARDED_FOR, FORWARDED_HOST, FORWARDED_METHOD, FORWARDED_PROTO, FORWARDED_URI,
    strip_incoming,
};
use crate::http::upstream::{connection_tokens, protected, strip_hop_by_hop};
use crate::http::{Body, full};

const ORIGINAL: [&str; 3] = ["x-original-url", "x-original-method", "x-original-uri"];

const FRAMING: [HeaderName; 4] = [
    header::CONTENT_LENGTH,
    header::TRANSFER_ENCODING,
    header::EXPECT,
    header::UPGRADE,
];

fn dashed(name: &HeaderName) -> String {
    name.as_str().replace('_', "-")
}

fn listed(name: &HeaderName, lists: &[&[HeaderName]]) -> bool {
    let n = dashed(name);
    lists.iter().flat_map(|l| l.iter()).any(|c| c.as_str() == n)
}

pub fn strip_visitor_copies(h: &mut HeaderMap, lists: &[&[HeaderName]]) {
    let tokens = connection_tokens(h);
    let upgrade = tokens.contains(&header::UPGRADE);
    for t in &tokens {
        if *t != header::UPGRADE && !protected(t) {
            h.remove(t);
        }
    }
    h.remove(header::CONNECTION);
    if upgrade {
        h.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
    }
    let names: Vec<HeaderName> = h.keys().filter(|n| listed(n, lists)).cloned().collect();
    for n in names {
        h.remove(n);
    }
}

pub struct Asked<'a> {
    pub method: &'a Method,
    pub host: &'a str,
    pub path_and_query: &'a str,
    pub client: &'a Client,
}

fn set(h: &mut HeaderMap, name: &'static str, value: &str) {
    if let Ok(v) = HeaderValue::from_str(value) {
        h.insert(HeaderName::from_static(name), v);
    }
}

fn origin_host(host: &str) -> String {
    if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

pub fn subrequest_headers(visitor: &HeaderMap, asked: &Asked<'_>) -> HeaderMap {
    let mut h = visitor.clone();
    strip_hop_by_hop(&mut h);
    for n in &FRAMING {
        h.remove(n);
    }
    strip_incoming(&mut h, false);
    let originals: Vec<HeaderName> = h
        .keys()
        .filter(|n| ORIGINAL.contains(&dashed(n).as_str()))
        .cloned()
        .collect();
    for n in originals {
        h.remove(n);
    }
    let proto = if asked.client.https { "https" } else { "http" };
    set(&mut h, FORWARDED_METHOD, asked.method.as_str());
    set(&mut h, FORWARDED_PROTO, proto);
    set(&mut h, FORWARDED_HOST, asked.host);
    set(&mut h, FORWARDED_URI, asked.path_and_query);
    set(&mut h, FORWARDED_FOR, &asked.client.forwarded_for);
    let url = format!(
        "{proto}://{}{}",
        origin_host(asked.host),
        asked.path_and_query
    );
    set(&mut h, "x-original-url", &url);
    set(&mut h, "x-original-uri", asked.path_and_query);
    set(&mut h, "x-original-method", asked.method.as_str());
    h
}

pub fn subrequest(path: &str, visitor: &HeaderMap, asked: &Asked<'_>) -> Option<Request<Body>> {
    let mut req = Request::get(path).body(full("")).ok()?;
    *req.headers_mut() = subrequest_headers(visitor, asked);
    Some(req)
}

pub fn copy_granted(from: &HeaderMap, to: &mut HeaderMap, copy: &[HeaderName]) {
    for name in copy {
        to.remove(name);
        for v in from.get_all(name) {
            to.append(name.clone(), v.clone());
        }
    }
}

#[cfg(test)]
mod tests;
