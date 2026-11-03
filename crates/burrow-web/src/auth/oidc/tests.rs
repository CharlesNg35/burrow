use super::*;

fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut h = HeaderMap::new();
    for (k, v) in pairs {
        h.insert(
            HeaderName::from_bytes(k.as_bytes()).unwrap(),
            HeaderValue::from_str(v).unwrap(),
        );
    }
    h
}

#[test]
fn a_browser_navigation_is_sent_to_log_in_and_scripts_and_tools_get_401() {
    let page = ("accept", "text/html,application/xhtml+xml,*/*;q=0.8");
    assert!(browser(&headers(&[page])));
    assert!(browser(&headers(&[page, ("sec-fetch-mode", "navigate")])));
    assert!(!browser(&headers(&[page, ("sec-fetch-mode", "cors")])));
    assert!(!browser(&headers(&[
        page,
        ("x-requested-with", "XMLHttpRequest")
    ])));
    assert!(!browser(&headers(&[("accept", "application/json")])));
    assert!(!browser(&headers(&[])));
}

#[test]
fn only_a_path_on_this_site_is_a_place_to_return_to() {
    assert_eq!(local_path("/wiki/a?b=1"), "/wiki/a?b=1");
    assert_eq!(local_path("//evil.example.com/"), "/");
    assert_eq!(local_path("/\\evil.example.com"), "/");
    assert_eq!(local_path("https://evil.example.com/"), "/");
    assert_eq!(local_path(""), "/");
}

#[test]
fn the_chooser_links_carry_the_provider_and_the_way_back_encoded() {
    assert_eq!(
        pages::start_link("google", "/a b?x=1&y=2"),
        "/.burrow/login?with=google&to=%2Fa+b%3Fx%3D1%26y%3D2"
    );
}
