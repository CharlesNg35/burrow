use burrow_syntax::address::url_host;
use http::header::{self, HeaderMap, HeaderValue};
use http::{Request, Response, Uri, Version};
use http_body_util::BodyExt;
use hyper_util::rt::TokioIo;
use tracing::debug;

use super::pool::{Back, H2, Ups};
use super::{Failed, Forward, extras, hop, refusal_page, rewrite_request, strip_hop_by_hop};
use crate::http::forwarded::Client;
use crate::http::site::Hop;
use crate::http::{Body, BoxError};
use crate::{BoxIo, Io, Upstream};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamReset(pub h2::Reason);

pub fn app_reset(e: &(dyn std::error::Error + 'static)) -> Option<h2::Reason> {
    let mut at = Some(e);
    while let Some(err) = at {
        if let Some(h2) = err.downcast_ref::<h2::Error>() {
            return (h2.is_reset() && h2.is_remote())
                .then(|| h2.reason())
                .flatten();
        }
        at = err.source();
    }
    None
}

fn wants_trailers(h: &HeaderMap) -> bool {
    crate::http::headers::has_token(h, header::TE, "trailers")
}

pub fn rewrite_for_h2<B>(req: &mut Request<B>, host: &str, client: &Client, https: bool) {
    let trailers = wants_trailers(req.headers());
    let crumbs: Vec<HeaderValue> = req
        .headers()
        .get_all(header::COOKIE)
        .iter()
        .cloned()
        .collect();
    rewrite_request(req, host, client);
    let h = req.headers_mut();
    h.remove(header::CONNECTION);
    h.remove(header::UPGRADE);
    h.remove(header::HOST);
    h.remove(header::COOKIE);
    for c in crumbs {
        h.append(header::COOKIE, c);
    }
    if trailers {
        h.insert(header::TE, HeaderValue::from_static("trailers"));
    }
    let scheme = if https { "https" } else { "http" };
    let pq = req.uri().path_and_query().map_or("/", |p| p.as_str());
    let absolute = format!("{scheme}://{}{pq}", url_host(host));
    *req.uri_mut() = absolute.parse().unwrap_or_else(|_| Uri::from_static("/"));
    *req.version_mut() = Version::HTTP_2;
}

pub(super) async fn handshake(io: BoxIo, how: &Hop) -> Result<H2, Failed> {
    let ending = io.ending();
    let io = hop(io, how, true).await?;
    let (tx, conn) = crate::http::h2::client()
        .handshake(TokioIo::new(io))
        .await
        .map_err(|e| {
            debug!("web: the HTTP/2 handshake with the app failed: {e}");
            Failed::Broken
        })?;
    let task = tokio::spawn(async move {
        if let Err(e) = conn.await {
            debug!("web: the HTTP/2 connection to the app ended: {e}");
        }
    });
    Ok(H2::new(tx, ending, task.abort_handle()))
}

pub(crate) async fn forward<U: Upstream>(
    up: &U,
    ups: &Ups,
    mut req: Request<Body>,
    f: Forward<'_>,
) -> Response<Body> {
    let visitor_h2 = req.version() == Version::HTTP_2;
    let https = matches!(f.site.hop, Hop::Tls(..));
    rewrite_for_h2(&mut req, &f.visitor.host, &f.client, https);
    extras(&mut req, &f);
    let deadline = tokio::time::Instant::now() + f.site.options.response_header_timeout;
    let asking = match f.asking(up, deadline) {
        Ok(a) => a,
        Err(why) => return f.page(refusal_page(why)),
    };
    let (mut resp, home, ticket) = match asking.send_h2(ups, req).await {
        Ok(r) => r,
        Err(miss) => {
            let mut page = f.page(miss.page);
            if let Some(reason) = miss.reset.filter(|_| visitor_h2) {
                page.extensions_mut().insert(StreamReset(reason));
            }
            return page;
        }
    };
    strip_hop_by_hop(resp.headers_mut());
    let lease = asking.lease;
    let lease = (lease, ticket);
    let activity = f.activity.clone();
    resp.map(move |b| {
        let b = b
            .map_err(move |e| {
                activity.fail();
                BoxError::from(e)
            })
            .boxed();
        Back::new(b, home, lease, None).boxed()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visitor() -> Client {
        Client {
            ip: "198.51.100.7".parse().unwrap(),
            trusted_peer: false,
            https: true,
            port: 443,
            forwarded_for: "198.51.100.7".into(),
        }
    }

    fn h2_request(uri: &str, headers: &[(&str, &str)]) -> Request<()> {
        let mut b = Request::get(uri).version(Version::HTTP_2);
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        b.body(()).unwrap()
    }

    #[test]
    fn an_h2_app_gets_an_absolute_uri_te_trailers_and_the_cookie_crumbs_as_sent() {
        let mut req = h2_request(
            "https://grpc.example.com/pkg.Svc/Call?x=1",
            &[("te", "trailers"), ("cookie", "a=1"), ("cookie", "b=2")],
        );
        rewrite_for_h2(&mut req, "grpc.example.com", &visitor(), false);
        assert_eq!(req.uri(), "http://grpc.example.com/pkg.Svc/Call?x=1");
        assert_eq!(req.version(), Version::HTTP_2);
        assert_eq!(req.headers()["te"], "trailers");
        let crumbs: Vec<_> = req.headers().get_all("cookie").iter().collect();
        assert_eq!(crumbs, ["a=1", "b=2"]);
        assert!(!req.headers().contains_key("host"), ":authority carries it");
        assert_eq!(req.headers()["x-forwarded-proto"], "https");
    }

    #[test]
    fn over_target_tls_the_scheme_is_https_and_an_ipv6_host_is_bracketed() {
        let mut req = h2_request("https://[2001:db8::1]/", &[]);
        rewrite_for_h2(&mut req, "2001:db8::1", &visitor(), true);
        assert_eq!(req.uri(), "https://[2001:db8::1]/");
        assert!(!req.headers().contains_key("te"));
    }

    #[test]
    fn an_http1_visitor_asking_for_trailers_keeps_only_that_token_and_no_upgrade() {
        let mut req = Request::get("/s")
            .header("host", "grpc.example.com")
            .header("te", "trailers, deflate")
            .header("connection", "upgrade")
            .header("upgrade", "websocket")
            .body(())
            .unwrap();
        rewrite_for_h2(&mut req, "grpc.example.com", &visitor(), false);
        assert_eq!(req.headers()["te"], "trailers");
        assert!(!req.headers().contains_key("upgrade"));
        assert!(!req.headers().contains_key("connection"));
        assert_eq!(req.uri(), "http://grpc.example.com/s");
    }

    #[test]
    fn only_a_reset_received_from_the_app_carries_its_reason() {
        let local = h2::Error::from(h2::Reason::CANCEL);
        assert_eq!(app_reset(&local), None, "not sent by the app");
        let io = std::io::Error::other("closed");
        assert_eq!(
            app_reset(&io),
            None,
            "a broken connection keeps the 502 page"
        );
    }
}
