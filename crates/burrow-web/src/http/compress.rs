use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use async_compression::tokio::bufread::GzipEncoder;
use bytes::BytesMut;
use http::header::{self, HeaderMap, HeaderValue};
use http::{Method, Response, StatusCode};
use http_body_util::{BodyDataStream, BodyExt};
use hyper::body::Frame;
use tokio_util::io::{StreamReader, poll_read_buf};

use super::headers::has_token;
use super::{Body, BoxError};

pub const MIN_SIZE: u64 = 1024;
const CHUNK: usize = 8 * 1024;

pub fn accepts_gzip(h: &HeaderMap) -> bool {
    let mut gzip = None;
    let mut any = None;
    for item in h
        .get_all(header::ACCEPT_ENCODING)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
    {
        let mut parts = item.split(';');
        let coding = parts.next().unwrap_or("").trim().to_ascii_lowercase();
        let q = parts
            .filter_map(|p| p.trim().strip_prefix("q=").or(p.trim().strip_prefix("Q=")))
            .find_map(|v| v.trim().parse::<f32>().ok())
            .unwrap_or(1.0);
        match coding.as_str() {
            "gzip" => gzip = Some(q > 0.0),
            "*" => any = Some(q > 0.0),
            _ => {}
        }
    }
    gzip.or(any).unwrap_or(false)
}

pub fn compressible_type(content_type: &str) -> bool {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if mime == "text/event-stream" {
        return false;
    }
    mime.starts_with("text/")
        || mime.ends_with("+json")
        || mime.ends_with("+xml")
        || matches!(
            mime.as_str(),
            "application/json"
                | "application/javascript"
                | "application/xml"
                | "application/wasm"
                | "image/svg+xml"
        )
}

fn no_transform(h: &HeaderMap) -> bool {
    has_token(h, header::CACHE_CONTROL, "no-transform")
}

fn small(h: &HeaderMap) -> bool {
    h.get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .is_some_and(|n| n < MIN_SIZE)
}

pub fn eligible(method: &Method, status: StatusCode, h: &HeaderMap) -> bool {
    method != Method::HEAD
        && status.is_success()
        && !matches!(status, StatusCode::NO_CONTENT | StatusCode::PARTIAL_CONTENT)
        && !h.contains_key(header::CONTENT_ENCODING)
        && !h.contains_key(header::CONTENT_RANGE)
        && !no_transform(h)
        && !small(h)
        && h.get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(compressible_type)
}

fn add_vary(h: &mut HeaderMap) {
    if !has_token(h, header::VARY, "*") && !has_token(h, header::VARY, "accept-encoding") {
        h.append(header::VARY, HeaderValue::from_static("accept-encoding"));
    }
}

fn weaken_etag(h: &mut HeaderMap) {
    let Some(tag) = h.get(header::ETAG).and_then(|v| v.to_str().ok()) else {
        return;
    };
    if tag.starts_with('"')
        && let Ok(v) = HeaderValue::from_str(&format!("W/{tag}"))
    {
        h.insert(header::ETAG, v);
    }
}

pub fn respond(method: &Method, gzip: bool, resp: Response<Body>) -> Response<Body> {
    if !eligible(method, resp.status(), resp.headers()) {
        return resp;
    }
    let (mut parts, body) = resp.into_parts();
    add_vary(&mut parts.headers);
    if !gzip {
        return Response::from_parts(parts, body);
    }
    parts.headers.remove(header::CONTENT_LENGTH);
    parts
        .headers
        .insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    weaken_etag(&mut parts.headers);
    Response::from_parts(parts, Gzipped::new(body).boxed())
}

type Encoder = GzipEncoder<StreamReader<BodyDataStream<IoBody>, bytes::Bytes>>;
type IoBody = http_body_util::combinators::MapErr<Body, fn(BoxError) -> io::Error>;

pub struct Gzipped {
    encoder: Pin<Box<Encoder>>,
    buf: BytesMut,
}

impl Gzipped {
    pub fn new(body: Body) -> Gzipped {
        let to_io: fn(BoxError) -> io::Error = io::Error::other;
        let reader = StreamReader::new(BodyDataStream::new(body.map_err(to_io)));
        Gzipped {
            encoder: Box::pin(GzipEncoder::new(reader)),
            buf: BytesMut::with_capacity(CHUNK),
        }
    }
}

impl hyper::body::Body for Gzipped {
    type Data = bytes::Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<bytes::Bytes>, BoxError>>> {
        let this = &mut *self;
        if this.buf.capacity() < CHUNK {
            this.buf.reserve(CHUNK);
        }
        match poll_read_buf(this.encoder.as_mut(), cx, &mut this.buf) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(0)) => Poll::Ready(None),
            Poll::Ready(Ok(_)) => Poll::Ready(Some(Ok(Frame::data(this.buf.split().freeze())))),
            Poll::Ready(Err(e)) => Poll::Ready(Some(Err(e.into()))),
        }
    }
}

#[cfg(test)]
mod tests;
