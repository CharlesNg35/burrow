use burrow_syntax::address::url_host;
use http::header::{self, HeaderMap, HeaderValue};
use http::{Request, Response, Uri, Version};
use http_body_util::BodyExt;
use hyper_util::rt::TokioIo;
use tracing::debug;

use super::pool::{H2App, Upstreams};
use super::{Failed, Forward, extras, hop, refusal_page, rewrite_request, strip_hop_by_hop};
use crate::http::forwarded::Client;
use crate::http::pages::Page;
use crate::http::site::Hop;
use crate::http::{Body, BoxError};
use crate::{Ending, Io, Upstream};

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

async fn open<U: Upstream>(up: &U, f: &Forward<'_>) -> Result<H2App, Failed> {
    let io = up
        .open(&f.site.service, &f.visitor)
        .await
        .map_err(Failed::Refused)?;
    let ending = io.ending();
    let io = hop(io, &f.site.hop, true).await?;
    let (tx, conn) = crate::http::h2::client()
        .handshake(TokioIo::new(io))
        .await
        .map_err(|e| {
            debug!("web: the HTTP/2 handshake with the app failed: {e}");
            Failed::Broken
        })?;
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            debug!("web: the HTTP/2 connection to the app ended: {e}");
        }
    });
    Ok(H2App {
        service: f.site.service.clone(),
        client: f.client.ip,
        tx,
        ending,
    })
}

pub(crate) async fn forward<U: Upstream>(
    up: &U,
    ups: &Upstreams,
    mut req: Request<Body>,
    f: Forward<'_>,
) -> Response<Body> {
    let visitor_h2 = req.version() == Version::HTTP_2;
    let https = matches!(f.site.hop, Hop::Tls(..));
    rewrite_for_h2(&mut req, &f.visitor.host, &f.client, https);
    extras(&mut req, &f);
    let service = f.site.service.as_str();
    let deadline = tokio::time::Instant::now() + f.site.options.response_header_timeout;
    let mut app = match ups.shared_h2(service, f.client.ip) {
        Some(app) => app,
        None => match tokio::time::timeout_at(deadline, open(up, &f)).await {
            Ok(Ok(app)) => {
                ups.keep_h2(app.clone());
                app
            }
            Ok(Err(Failed::Refused(why))) => return f.page(refusal_page(why)),
            Ok(Err(Failed::Broken)) => return f.page(Page::BadGateway),
            Err(_) => return f.page(Page::Timeout),
        },
    };
    let sent = async {
        app.tx.ready().await?;
        app.tx.send_request(req).await
    };
    let mut resp = match tokio::time::timeout_at(deadline, sent).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            debug!(peer = %f.visitor.addr, "web: service \"{service}\" did not answer over HTTP/2: {e}");
            let why = app.ending.as_ref().and_then(Ending::refused);
            let mut page = f.page(why.map_or(Page::BadGateway, refusal_page));
            if let Some(reason) = app_reset(&e).filter(|_| visitor_h2) {
                page.extensions_mut().insert(StreamReset(reason));
            }
            return page;
        }
        Err(_) => return f.page(Page::Timeout),
    };
    strip_hop_by_hop(resp.headers_mut());
    let activity = f.activity.clone();
    resp.map(move |b| {
        b.map_err(move |e| {
            activity.fail();
            BoxError::from(e)
        })
        .boxed()
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
