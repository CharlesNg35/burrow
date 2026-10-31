use bytes::Bytes;
use http::header::{ACCEPT, CONNECTION, HOST, USER_AGENT};
use http::uri::PathAndQuery;
use http::{HeaderValue, Request, Response, StatusCode, Uri, Version};
use http_body_util::Empty;
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;

use super::site::{Hop, hop};
use crate::{BoxIo, SiteOptions};

pub struct HealthCheck {
    hop: Result<Hop, String>,
    host: HeaderValue,
    path: PathAndQuery,
    agent: HeaderValue,
    http2: bool,
}

impl std::fmt::Debug for HealthCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HealthCheck")
            .field("host", &self.host)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Ask<'a> {
    pub host: &'a str,
    pub path: &'a str,
    pub agent: &'a str,
}

pub fn origin_form(path: &str) -> Result<PathAndQuery, String> {
    let bad = || format!("\"{path}\" is not a path; write it like /healthz");
    if !path.starts_with('/') || path.contains('#') {
        return Err(bad());
    }
    path.parse::<PathAndQuery>().map_err(|_| bad())
}

pub fn verdict(status: StatusCode) -> Result<(), String> {
    if (200..400).contains(&status.as_u16()) {
        Ok(())
    } else {
        Err(format!("answered {status}"))
    }
}

fn no_http(e: &hyper::Error) -> String {
    format!("the app did not answer HTTP ({e})")
}

impl HealthCheck {
    pub fn build(site: &SiteOptions, ask: &Ask<'_>) -> Result<HealthCheck, String> {
        let Ask { host, path, agent } = *ask;
        let host = burrow_syntax::address::url_host(host);
        Ok(HealthCheck {
            hop: hop(site),
            host: HeaderValue::from_str(&host)
                .map_err(|_| format!("\"{host}\" is not a host name"))?,
            path: origin_form(path)?,
            agent: HeaderValue::from_str(agent)
                .map_err(|_| format!("\"{agent}\" is not a User-Agent"))?,
            http2: site.target_http2,
        })
    }

    fn request(&self) -> Request<Empty<Bytes>> {
        let mut req = Request::new(Empty::new());
        let h = req.headers_mut();
        h.insert(USER_AGENT, self.agent.clone());
        h.insert(ACCEPT, HeaderValue::from_static("*/*"));
        req
    }

    fn h1_request(&self) -> Request<Empty<Bytes>> {
        let mut req = self.request();
        *req.uri_mut() = self.path.clone().into();
        let h = req.headers_mut();
        h.insert(HOST, self.host.clone());
        h.insert(CONNECTION, HeaderValue::from_static("close"));
        req
    }

    fn h2_request(&self) -> Result<Request<Empty<Bytes>>, String> {
        let scheme = if matches!(self.hop, Ok(Hop::Tls(..))) {
            "https"
        } else {
            "http"
        };
        let host = self.host.to_str().unwrap_or_default();
        let uri: Uri = format!("{scheme}://{host}{}", self.path)
            .parse()
            .map_err(|_| format!("\"{host}\" is not a host name"))?;
        let mut req = self.request();
        *req.uri_mut() = uri;
        *req.version_mut() = Version::HTTP_2;
        Ok(req)
    }

    async fn secure(&self, io: BoxIo) -> Result<BoxIo, String> {
        match &self.hop {
            Ok(how) => super::upstream::hop(io, how, self.http2)
                .await
                .map_err(|e| e.to_string()),
            Err(e) => Err(format!("target_tls: {e}")),
        }
    }

    pub async fn run(&self, io: BoxIo) -> Result<(), String> {
        let io = TokioIo::new(self.secure(io).await?);
        let status = if self.http2 {
            self.over_h2(io).await?
        } else {
            self.over_h1(io).await?
        };
        verdict(status)
    }

    async fn over_h1(&self, io: TokioIo<BoxIo>) -> Result<StatusCode, String> {
        let (mut tx, conn) = super::h2::app_http1()
            .handshake(io)
            .await
            .map_err(|e| no_http(&e))?;
        answered(tx.send_request(self.h1_request()), conn).await
    }

    async fn over_h2(&self, io: TokioIo<BoxIo>) -> Result<StatusCode, String> {
        let req = self.h2_request()?;
        let (mut tx, conn) = super::h2::client()
            .handshake(io)
            .await
            .map_err(|e| no_http(&e))?;
        answered(tx.send_request(req), conn).await
    }
}

async fn answered<S, C>(send: S, conn: C) -> Result<StatusCode, String>
where
    S: Future<Output = hyper::Result<Response<Incoming>>>,
    C: Future<Output = hyper::Result<()>>,
{
    tokio::pin!(conn, send);
    let answer = tokio::select! {
        biased;
        r = &mut send => r,
        r = &mut conn => match r {
            Ok(()) => send.await,
            Err(e) => return Err(no_http(&e)),
        },
    };
    Ok(answer.map_err(|e| no_http(&e))?.status())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_hundreds_and_three_hundreds_are_up_and_the_rest_down() {
        for up in [200, 204, 299, 300, 302, 399] {
            assert_eq!(verdict(StatusCode::from_u16(up).unwrap()), Ok(()), "{up}");
        }
        for down in [101, 199, 400, 401, 403, 404, 500, 503] {
            assert!(
                verdict(StatusCode::from_u16(down).unwrap()).is_err(),
                "{down}"
            );
        }
        assert_eq!(
            verdict(StatusCode::UNAUTHORIZED).unwrap_err(),
            "answered 401 Unauthorized"
        );
    }

    #[test]
    fn an_ipv6_host_is_sent_in_brackets() {
        let ask = |host| Ask {
            host,
            path: "/",
            agent: "a",
        };
        let o = SiteOptions::default();
        assert_eq!(
            HealthCheck::build(&o, &ask("2001:db8::1")).unwrap().host,
            "[2001:db8::1]"
        );
        assert_eq!(
            HealthCheck::build(&o, &ask("[2001:db8::1]")).unwrap().host,
            "[2001:db8::1]"
        );
        assert_eq!(
            HealthCheck::build(&o, &ask("10.0.0.1")).unwrap().host,
            "10.0.0.1"
        );
        assert!(HealthCheck::build(&o, &ask("a\nb")).is_err());
    }

    #[test]
    fn the_path_is_the_origin_form_of_a_request() {
        assert_eq!(origin_form("/healthz").unwrap(), "/healthz");
        assert_eq!(origin_form("/v1/ready?full=1").unwrap(), "/v1/ready?full=1");
        for bad in ["healthz", "http://app/healthz", "/a#b", "/a b", ""] {
            assert!(origin_form(bad).is_err(), "{bad}");
        }
    }
}
