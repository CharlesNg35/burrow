use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use bytes::Bytes;
use http::{HeaderValue, Response, StatusCode, header};

use super::{Body, full};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Page {
    BadRequest,
    Forbidden,
    NotFound,
    Misdirected,
    TooLarge,
    HeadersTooLarge,
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
            Page::HeadersTooLarge => StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE,
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
            Page::HeadersTooLarge => (
                "Too many headers",
                "The browser sent more request headers than this site takes.",
            ),
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
        self.answer(Bytes::from(self.html()))
    }

    pub fn response_from(self, pages: &ErrorPages) -> Response<Body> {
        match pages.0.get(&self) {
            Some(html) => self.answer(html.clone()),
            None => self.response(),
        }
    }

    fn answer(self, html: Bytes) -> Response<Body> {
        let mut r = Response::new(full(html));
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

pub const MAX_PAGE: u64 = 1024 * 1024;

pub const PAGE_FILES: [(&str, &[Page]); 7] = [
    ("404.html", &[Page::NotFound]),
    ("413.html", &[Page::TooLarge]),
    ("429.html", &[Page::TooMany]),
    ("502.html", &[Page::BadGateway]),
    ("503.html", &[Page::Unavailable, Page::Busy]),
    ("504.html", &[Page::Timeout]),
    ("maintenance.html", &[Page::Maintenance]),
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ErrorPages(HashMap<Page, Bytes>);

impl ErrorPages {
    pub fn load(dir: &Path) -> Result<ErrorPages, String> {
        let mut pages = HashMap::new();
        for (file, which) in PAGE_FILES {
            let path = dir.join(file);
            let Ok(f) = std::fs::File::open(&path) else {
                continue;
            };
            let mut html = Vec::new();
            f.take(MAX_PAGE + 1)
                .read_to_end(&mut html)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            if u64::try_from(html.len()).unwrap_or(u64::MAX) > MAX_PAGE {
                return Err(format!("{} is larger than 1 MiB", path.display()));
            }
            let html = Bytes::from(html);
            for page in which {
                pages.insert(*page, html.clone());
            }
        }
        Ok(ErrorPages(pages))
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
            (Page::HeadersTooLarge, 431),
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

    async fn text(r: Response<Body>) -> Bytes {
        http_body_util::BodyExt::collect(r.into_body())
            .await
            .unwrap()
            .to_bytes()
    }

    #[tokio::test]
    async fn pages_from_the_folder_replace_the_built_in_ones_they_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("404.html"), "<p>gone</p>").unwrap();
        std::fs::write(dir.path().join("503.html"), "<p>later</p>").unwrap();
        std::fs::write(dir.path().join("maintenance.html"), "<p>soon</p>").unwrap();
        let pages = ErrorPages::load(dir.path()).unwrap();
        let r = Page::Busy.response_from(&pages);
        assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(r.headers()[header::RETRY_AFTER], "10");
        assert_eq!(
            r.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        assert_eq!(text(r).await, "<p>later</p>");
        let got = |p: Page| text(p.response_from(&pages));
        assert_eq!(got(Page::NotFound).await, "<p>gone</p>");
        assert_eq!(got(Page::Unavailable).await, "<p>later</p>");
        assert_eq!(got(Page::Maintenance).await, "<p>soon</p>");
        assert_eq!(got(Page::BadGateway).await, Page::BadGateway.html());
        assert_eq!(got(Page::BadRequest).await, Page::BadRequest.html());
    }

    #[test]
    fn a_page_over_one_mebibyte_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("502.html"), vec![b'a'; 1024 * 1024 + 1]).unwrap();
        let e = ErrorPages::load(dir.path()).unwrap_err();
        assert!(e.ends_with("502.html is larger than 1 MiB"), "{e}");
        assert_eq!(
            ErrorPages::load(&dir.path().join("none")).unwrap(),
            ErrorPages::default()
        );
    }
}
