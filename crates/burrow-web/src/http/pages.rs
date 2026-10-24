use bytes::Bytes;
use http::{HeaderValue, Response, StatusCode, header};

use super::{Body, full};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    BadRequest,
    Forbidden,
    NotFound,
    Misdirected,
    TooLarge,
    TooMany,
    BadGateway,
    Unavailable,
    Busy,
    Pending,
    Maintenance,
    Timeout,
}

pub const RETRY_AFTER_SECS: u32 = 10;

impl Page {
    pub fn status(self) -> StatusCode {
        match self {
            Page::BadRequest => StatusCode::BAD_REQUEST,
            Page::Forbidden => StatusCode::FORBIDDEN,
            Page::NotFound => StatusCode::NOT_FOUND,
            Page::Misdirected => StatusCode::MISDIRECTED_REQUEST,
            Page::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Page::TooMany => StatusCode::TOO_MANY_REQUESTS,
            Page::BadGateway => StatusCode::BAD_GATEWAY,
            Page::Unavailable | Page::Busy | Page::Pending | Page::Maintenance => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            Page::Timeout => StatusCode::GATEWAY_TIMEOUT,
        }
    }

    fn text(self) -> (&'static str, &'static str) {
        match self {
            Page::BadRequest => (
                "Bad request",
                "The browser sent a request this site cannot read.",
            ),
            Page::Forbidden => (
                "Not allowed",
                "This site does not take visitors from your address.",
            ),
            Page::NotFound => ("Not found", "There is no site at this address."),
            Page::Misdirected => (
                "Wrong site",
                "This connection belongs to another site; open the address again.",
            ),
            Page::TooLarge => ("Too large", "The upload is too large for this site."),
            Page::TooMany => (
                "Too many requests",
                "Too many requests from your address; wait a moment.",
            ),
            Page::BadGateway => ("Bad gateway", "The site's app did not answer properly."),
            Page::Unavailable => (
                "Unavailable",
                "The site's app cannot be reached right now; try again soon.",
            ),
            Page::Busy => (
                "Busy",
                "The site is busy right now; try again in a few seconds.",
            ),
            Page::Pending => (
                "Almost ready",
                "This site's certificate is on its way; try again in a minute.",
            ),
            Page::Maintenance => ("Maintenance", "Back soon: this site is being updated."),
            Page::Timeout => ("Timed out", "The site's app did not answer in time."),
        }
    }

    fn retry_after(self) -> Option<u32> {
        match self {
            Page::Busy | Page::TooMany => Some(RETRY_AFTER_SECS),
            Page::Pending | Page::Maintenance => Some(60),
            _ => None,
        }
    }

    pub fn html(self) -> String {
        let (title, line) = self.text();
        format!(
            "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>{} {title}</title></head>\n<body style=\"font-family:system-ui,sans-serif;max-width:32rem;margin:4rem auto;padding:0 1rem\"><h1>{title}</h1><p>{line}</p></body></html>\n",
            self.status().as_u16()
        )
    }

    pub fn response(self) -> Response<Body> {
        let mut r = Response::new(full(Bytes::from(self.html())));
        *r.status_mut() = self.status();
        let h = r.headers_mut();
        h.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        if let Some(secs) = self.retry_after() {
            h.insert(header::RETRY_AFTER, HeaderValue::from(secs));
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_built_in_page_has_its_status_and_a_short_html_body() {
        let all = [
            (Page::NotFound, 404),
            (Page::TooLarge, 413),
            (Page::TooMany, 429),
            (Page::BadGateway, 502),
            (Page::Unavailable, 503),
            (Page::Busy, 503),
            (Page::Pending, 503),
            (Page::Maintenance, 503),
            (Page::Timeout, 504),
            (Page::BadRequest, 400),
            (Page::Forbidden, 403),
            (Page::Misdirected, 421),
        ];
        for (page, code) in all {
            let r = page.response();
            assert_eq!(r.status().as_u16(), code, "{page:?}");
            assert_eq!(
                r.headers()[header::CONTENT_TYPE],
                "text/html; charset=utf-8"
            );
            assert_eq!(r.headers()[header::CACHE_CONTROL], "no-store");
            let html = page.html();
            assert!(
                html.starts_with("<!doctype html>") && html.len() < 600,
                "{page:?}"
            );
            assert!(html.contains(&format!("<title>{code} ")), "{page:?}");
        }
    }

    #[test]
    fn busy_rate_limited_pending_and_maintenance_pages_say_when_to_retry() {
        let after = |p: Page| p.response().headers().get(header::RETRY_AFTER).cloned();
        assert_eq!(after(Page::Busy).unwrap(), "10");
        assert_eq!(after(Page::TooMany).unwrap(), "10");
        assert_eq!(after(Page::Pending).unwrap(), "60");
        assert_eq!(after(Page::Maintenance).unwrap(), "60");
        assert_eq!(after(Page::Unavailable), None);
        assert_eq!(after(Page::BadGateway), None);
    }
}
