use super::*;

#[test]
fn the_accept_key_is_the_one_of_rfc_6455_section_1_3() {
    assert_eq!(
        accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
        "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
    );
}

fn request(
    path: &str,
    edit: impl FnOnce(http::request::Builder) -> http::request::Builder,
) -> Request<()> {
    let b = Request::builder()
        .method(Method::GET)
        .uri(path)
        .version(Version::HTTP_11)
        .header(header::HOST, "site.example.com")
        .header(header::UPGRADE, "websocket")
        .header(header::CONNECTION, "keep-alive, Upgrade")
        .header(header::SEC_WEBSOCKET_VERSION, "13")
        .header(header::SEC_WEBSOCKET_KEY, "dGhlIHNhbXBsZSBub25jZQ==");
    edit(b).body(()).unwrap()
}

#[test]
fn only_a_websocket_upgrade_on_the_exact_path_reaches_the_tunnel() {
    let ok = Answer::Upgrade("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=".into());
    assert_eq!(answer(&request("/burrow", |b| b), "/burrow"), ok);
    assert_eq!(answer(&request("/burrow?x=1", |b| b), "/burrow"), ok);
    assert_eq!(
        answer(&request("/other", |b| b), "/burrow"),
        Answer::NotFound
    );
    assert_eq!(
        answer(&request("/burrowx", |b| b), "/burrow"),
        Answer::NotFound
    );
    assert_eq!(
        answer(&request("/burrow/deeper", |b| b), "/burrow"),
        Answer::UpgradeRequired
    );
    let post = request("/burrow", |b| b.method(Method::POST));
    assert_eq!(answer(&post, "/burrow"), Answer::UpgradeRequired);
    let http10 = request("/burrow", |b| b.version(Version::HTTP_10));
    assert_eq!(answer(&http10, "/burrow"), Answer::UpgradeRequired);
    let browser = Request::builder().uri("/burrow").body(()).unwrap();
    assert_eq!(answer(&browser, "/burrow"), Answer::UpgradeRequired);
}

#[test]
fn a_wrong_version_asks_for_13_and_a_bad_key_is_a_bad_request() {
    let mut v12 = request("/burrow", |b| b);
    v12.headers_mut().insert(
        header::SEC_WEBSOCKET_VERSION,
        HeaderValue::from_static("12"),
    );
    assert_eq!(answer(&v12, "/burrow"), Answer::UpgradeRequired);
    let r = reply(&Answer::UpgradeRequired);
    assert_eq!(r.status(), StatusCode::UPGRADE_REQUIRED);
    assert_eq!(r.headers()[header::SEC_WEBSOCKET_VERSION], "13");
    let mut short = request("/burrow", |b| b);
    short.headers_mut().insert(
        header::SEC_WEBSOCKET_KEY,
        HeaderValue::from_static("c2hvcnQ="),
    );
    assert_eq!(answer(&short, "/burrow"), Answer::BadRequest);
    let mut none = request("/burrow", |b| b);
    none.headers_mut().remove(header::SEC_WEBSOCKET_KEY);
    assert_eq!(answer(&none, "/burrow"), Answer::BadRequest);
    let mut no_conn = request("/burrow", |b| b);
    no_conn
        .headers_mut()
        .insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
    assert_eq!(answer(&no_conn, "/burrow"), Answer::UpgradeRequired);
}

#[test]
fn the_switching_answer_carries_the_accept_header() {
    let r = reply(&Answer::Upgrade("abc".into()));
    assert_eq!(r.status(), StatusCode::SWITCHING_PROTOCOLS);
    assert_eq!(r.headers()[header::UPGRADE], "websocket");
    assert_eq!(r.headers()[header::CONNECTION], "Upgrade");
    assert_eq!(r.headers()[header::SEC_WEBSOCKET_ACCEPT], "abc");
    assert_eq!(reply(&Answer::NotFound).status(), StatusCode::NOT_FOUND);
}
