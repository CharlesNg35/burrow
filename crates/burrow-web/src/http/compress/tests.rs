use std::io::Read;

use super::*;
use crate::http::full;

fn ae(v: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(header::ACCEPT_ENCODING, HeaderValue::from_str(v).unwrap());
    h
}

#[test]
fn gzip_is_used_only_when_the_client_accepts_it_with_a_weight_above_zero() {
    assert!(accepts_gzip(&ae("gzip")));
    assert!(accepts_gzip(&ae("br, GZIP;q=0.5")));
    assert!(accepts_gzip(&ae("*")));
    assert!(accepts_gzip(&ae("identity, *;q=0.1")));
    assert!(!accepts_gzip(&ae("gzip;q=0")));
    assert!(!accepts_gzip(&ae("gzip; q=0.000, *")));
    assert!(!accepts_gzip(&ae("br")));
    assert!(!accepts_gzip(&ae("*;q=0")));
    assert!(!accepts_gzip(&ae("identity")));
    assert!(!accepts_gzip(&HeaderMap::new()));
}

#[test]
fn only_text_like_types_are_compressible() {
    for yes in [
        "text/html; charset=utf-8",
        "text/css",
        "application/json",
        "application/problem+json",
        "application/javascript",
        "application/xml",
        "application/atom+xml",
        "image/svg+xml",
        "application/wasm",
        "TEXT/PLAIN",
    ] {
        assert!(compressible_type(yes), "{yes}");
    }
    for no in [
        "text/event-stream",
        "image/png",
        "application/octet-stream",
        "video/mp4",
        "application/zip",
        "",
    ] {
        assert!(!compressible_type(no), "{no}");
    }
}

fn headers(pairs: &[(header::HeaderName, &str)]) -> HeaderMap {
    let mut h = HeaderMap::new();
    for (k, v) in pairs {
        h.append(k.clone(), HeaderValue::from_str(v).unwrap());
    }
    h
}

#[test]
fn heads_empty_partial_unchanged_encoded_small_and_no_transform_responses_are_left_alone() {
    let html = (header::CONTENT_TYPE, "text/html");
    let get = Method::GET;
    assert!(eligible(
        &get,
        StatusCode::OK,
        &headers(std::slice::from_ref(&html))
    ));
    assert!(!eligible(
        &Method::HEAD,
        StatusCode::OK,
        &headers(std::slice::from_ref(&html))
    ));
    for status in [
        StatusCode::NO_CONTENT,
        StatusCode::PARTIAL_CONTENT,
        StatusCode::NOT_MODIFIED,
        StatusCode::SWITCHING_PROTOCOLS,
    ] {
        assert!(
            !eligible(&get, status, &headers(std::slice::from_ref(&html))),
            "{status}"
        );
    }
    for extra in [
        (header::CONTENT_ENCODING, "br"),
        (header::CONTENT_RANGE, "bytes 0-1/2"),
        (header::CACHE_CONTROL, "public, No-Transform"),
        (header::CONTENT_LENGTH, "1023"),
    ] {
        let h = headers(&[html.clone(), extra.clone()]);
        assert!(!eligible(&get, StatusCode::OK, &h), "{extra:?}");
    }
    let big = headers(&[html.clone(), (header::CONTENT_LENGTH, "1024")]);
    assert!(eligible(&get, StatusCode::OK, &big));
    assert!(!eligible(&get, StatusCode::OK, &headers(&[])));
}

fn page(len: usize, extra: &[(header::HeaderName, &str)]) -> Response<Body> {
    let mut r = Response::new(full("a".repeat(len)));
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    for (k, v) in extra {
        h.append(k.clone(), HeaderValue::from_str(v).unwrap());
    }
    r
}

#[tokio::test]
async fn a_compressed_response_is_gzip_with_vary_a_weak_etag_and_no_length() {
    let r = respond(
        &Method::GET,
        true,
        page(5000, &[(header::ETAG, "\"v1\""), (header::VARY, "Origin")]),
    );
    let h = r.headers().clone();
    assert_eq!(h[header::CONTENT_ENCODING], "gzip");
    assert!(!h.contains_key(header::CONTENT_LENGTH));
    assert_eq!(h[header::ETAG], "W/\"v1\"");
    let vary: Vec<_> = h.get_all(header::VARY).iter().collect();
    assert_eq!(vary, ["Origin", "accept-encoding"]);
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    assert!(bytes.len() < 200);
    let mut out = String::new();
    flate2::read::GzDecoder::new(&bytes[..])
        .read_to_string(&mut out)
        .unwrap();
    assert_eq!(out, "a".repeat(5000));
}

#[tokio::test]
async fn a_client_without_gzip_gets_the_body_as_is_with_vary() {
    let r = respond(&Method::GET, false, page(5000, &[(header::ETAG, "\"v1\"")]));
    let h = r.headers().clone();
    assert!(!h.contains_key(header::CONTENT_ENCODING));
    assert_eq!(h[header::CONTENT_LENGTH], "5000");
    assert_eq!(h[header::ETAG], "\"v1\"");
    assert_eq!(h[header::VARY], "accept-encoding");
    let small = respond(&Method::GET, true, page(10, &[]));
    assert!(!small.headers().contains_key(header::VARY));
    let weak = respond(&Method::GET, true, page(5000, &[(header::ETAG, "W/\"x\"")]));
    assert_eq!(weak.headers()[header::ETAG], "W/\"x\"");
    let star = respond(&Method::GET, true, page(5000, &[(header::VARY, "*")]));
    assert_eq!(star.headers().get_all(header::VARY).iter().count(), 1);
}
