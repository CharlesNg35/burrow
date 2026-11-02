use burrow_syntax::web::oidc::{LOGIN_PATH, LOGOUT_PATH};
use http::{Response, StatusCode};

use crate::http::Body;
use crate::http::pages::{escape, html, html_response};

fn page(status: StatusCode, title: &str, line: &str) -> Response<Body> {
    html_response(status, html(status, title, line))
}

fn encoded(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

pub(crate) fn start_link(provider: &str, to: &str) -> String {
    format!("{LOGIN_PATH}?with={}&to={}", encoded(provider), encoded(to))
}

pub(crate) fn chooser(providers: &[String], to: &str) -> Response<Body> {
    let links: Vec<String> = providers
        .iter()
        .map(|p| {
            format!(
                "<a href=\"{}\">Log in with {}</a>",
                escape(&start_link(p, to)),
                escape(p)
            )
        })
        .collect();
    page(StatusCode::OK, "Log in", &links.join("<br>"))
}

pub(crate) fn login_first() -> Response<Body> {
    page(
        StatusCode::UNAUTHORIZED,
        "Login needed",
        "This site needs you to log in; open it in a browser.",
    )
}

pub(crate) fn not_allowed(email: &str) -> Response<Body> {
    page(
        StatusCode::FORBIDDEN,
        "Not allowed",
        &format!(
            "{} may not use this site. <a href=\"{LOGOUT_PATH}\">Log out</a> to use another account.",
            escape(email)
        ),
    )
}

pub(crate) fn failed() -> Response<Body> {
    page(
        StatusCode::BAD_GATEWAY,
        "Login failed",
        "The login provider did not answer properly; <a href=\"/\">try again</a> soon.",
    )
}

pub(crate) fn stale() -> Response<Body> {
    page(
        StatusCode::BAD_REQUEST,
        "Login expired",
        "This login took too long or began in another browser; <a href=\"/\">start again</a>.",
    )
}

pub(crate) fn cancelled(said: &str) -> Response<Body> {
    page(
        StatusCode::FORBIDDEN,
        "Login not completed",
        &format!(
            "The login provider said: {}. <a href=\"/\">Try again</a>.",
            escape(said)
        ),
    )
}

pub(crate) fn logged_out() -> Response<Body> {
    page(
        StatusCode::OK,
        "Logged out",
        "You are logged out. <a href=\"/\">Log in again</a>.",
    )
}

pub(crate) fn not_found() -> Response<Body> {
    crate::http::pages::Page::NotFound.response()
}
