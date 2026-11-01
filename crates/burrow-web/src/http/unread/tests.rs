use std::collections::VecDeque;

use bytes::Bytes;
use http_body_util::BodyExt;

use super::*;
use crate::http::full;

#[derive(Clone, Copy)]
enum End {
    Done,
    Stall,
    Broken,
}

struct Test {
    frames: VecDeque<Bytes>,
    end: End,
    exact: Option<u64>,
}

fn body(sizes: &[usize], end: End) -> Test {
    Test {
        frames: sizes.iter().map(|n| Bytes::from(vec![b'x'; *n])).collect(),
        end,
        exact: None,
    }
}

impl HttpBody for Test {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, std::io::Error>>> {
        let this = self.get_mut();
        match (this.frames.pop_front(), this.end) {
            (Some(b), _) => Poll::Ready(Some(Ok(Frame::data(b)))),
            (None, End::Done) => Poll::Ready(None),
            (None, End::Stall) => Poll::Pending,
            (None, End::Broken) => Poll::Ready(Some(Err(std::io::Error::other("cut off")))),
        }
    }

    fn is_end_stream(&self) -> bool {
        self.frames.is_empty() && matches!(self.end, End::Done)
    }

    fn size_hint(&self) -> SizeHint {
        self.exact
            .map_or_else(SizeHint::default, SizeHint::with_exact)
    }
}

fn request(b: Test) -> Request<Test> {
    Request::post("/").body(b).unwrap()
}

fn answer(status: StatusCode) -> Response<Body> {
    let mut r = Response::new(full("no"));
    *r.status_mut() = status;
    r
}

async fn settled(b: Test, status: StatusCode) -> Response<Body> {
    let (req, unread) = keep(request(b));
    drop(req);
    let mut r = answer(status);
    unread.unwrap().settle(&mut r).await;
    r
}

fn closes(r: &Response<Body>) -> bool {
    has_token(r.headers(), header::CONNECTION, "close")
}

#[tokio::test]
async fn a_body_nobody_read_comes_back_and_a_read_one_does_not() {
    let (req, unread) = keep(request(body(&[10], End::Done)));
    drop(req);
    assert!(unread.unwrap().0.lock().unwrap().is_some());
    let (req, unread) = keep(request(body(&[10], End::Done)));
    let got = req.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(got.len(), 10);
    assert!(unread.unwrap().0.lock().unwrap().is_none());
}

#[test]
fn only_an_unfinished_http_1_1_body_without_expect_or_close_gets_a_slot() {
    assert!(keep(request(body(&[1], End::Done))).1.is_some());
    assert!(keep(request(body(&[], End::Done))).1.is_none());
    let mut old = request(body(&[1], End::Done));
    *old.version_mut() = Version::HTTP_10;
    assert!(keep(old).1.is_none());
    let mut h2 = request(body(&[1], End::Done));
    *h2.version_mut() = Version::HTTP_2;
    assert!(keep(h2).1.is_none());
    for (name, value) in [
        ("expect", "100-continue"),
        ("connection", "Keep-Alive, Close"),
    ] {
        let mut r = request(body(&[1], End::Done));
        r.headers_mut()
            .insert(name, HeaderValue::from_static(value));
        assert!(keep(r).1.is_none(), "{name}");
    }
}

#[tokio::test]
async fn a_small_body_is_dropped_and_the_answer_keeps_the_connection() {
    let r = settled(body(&[4096, 4096], End::Done), StatusCode::UNAUTHORIZED).await;
    assert!(!closes(&r));
    let whole = vec![64 * 1024; 4];
    let r = settled(body(&whole, End::Done), StatusCode::FORBIDDEN).await;
    assert!(!closes(&r));
}

#[tokio::test]
async fn a_body_past_256_kib_cut_off_or_broken_closes_the_connection() {
    let over = vec![64 * 1024; 5];
    assert!(closes(
        &settled(body(&over, End::Done), StatusCode::UNAUTHORIZED).await
    ));
    let r = settled(body(&[10], End::Broken), StatusCode::UNAUTHORIZED).await;
    assert!(closes(&r));
    let mut declared = body(&[], End::Stall);
    declared.exact = Some(DISCARD_BYTES + 1);
    let started = std::time::Instant::now();
    assert!(closes(&settled(declared, StatusCode::UNAUTHORIZED).await));
    assert!(started.elapsed() < Duration::from_millis(500));
}

#[tokio::test(start_paused = true)]
async fn a_stalled_body_closes_the_connection_after_one_second() {
    let started = tokio::time::Instant::now();
    let r = settled(body(&[10], End::Stall), StatusCode::UNAUTHORIZED).await;
    assert!(closes(&r));
    assert_eq!(started.elapsed(), DISCARD_TIME);
}

#[tokio::test]
async fn every_early_answer_drains_including_400_413_and_431() {
    for status in [
        StatusCode::BAD_REQUEST,
        StatusCode::PAYLOAD_TOO_LARGE,
        StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE,
        StatusCode::SERVICE_UNAVAILABLE,
    ] {
        let r = settled(body(&[4096], End::Done), status).await;
        assert!(!closes(&r), "{status}");
    }
}

#[tokio::test]
async fn an_upgrade_or_an_answer_that_already_closes_is_left_alone() {
    let r = settled(body(&[10], End::Stall), StatusCode::SWITCHING_PROTOCOLS).await;
    assert!(r.headers().get(header::CONNECTION).is_none());
    let (req, unread) = keep(request(body(&[10], End::Stall)));
    drop(req);
    let mut r = answer(StatusCode::OK);
    r.headers_mut()
        .insert(header::CONNECTION, HeaderValue::from_static("close, x-y"));
    unread.unwrap().settle(&mut r).await;
    assert_eq!(r.headers()[header::CONNECTION], "close, x-y");
}
