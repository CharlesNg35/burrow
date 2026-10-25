use bytes::Bytes;
use http::header::{ACCEPT, CONNECTION, HOST, USER_AGENT};
use http::uri::PathAndQuery;
use http::{HeaderValue, Request, StatusCode};
use http_body_util::Empty;
use hyper_util::rt::TokioIo;

use super::site::{Hop, hop};
use super::{MAX_HEAD, MAX_HEADERS};
use crate::{BoxIo, SiteOptions};

pub struct HealthCheck {
    hop: Result<Hop, String>,
    host: HeaderValue,
    path: PathAndQuery,
    agent: HeaderValue,
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
        let host = match host.parse::<std::net::Ipv6Addr>() {
            Ok(ip) => format!("[{ip}]"),
            Err(_) => host.to_string(),
        };
        Ok(HealthCheck {
            hop: hop(site),
            host: HeaderValue::from_str(&host)
                .map_err(|_| format!("\"{host}\" is not a host name"))?,
            path: origin_form(path)?,
            agent: HeaderValue::from_str(agent)
                .map_err(|_| format!("\"{agent}\" is not a User-Agent"))?,
        })
    }

    fn request(&self) -> Request<Empty<Bytes>> {
        let mut req = Request::new(Empty::new());
        *req.uri_mut() = self.path.clone().into();
        let h = req.headers_mut();
        h.insert(HOST, self.host.clone());
        h.insert(USER_AGENT, self.agent.clone());
        h.insert(ACCEPT, HeaderValue::from_static("*/*"));
        h.insert(CONNECTION, HeaderValue::from_static("close"));
        req
    }

    async fn secure(&self, io: BoxIo) -> Result<BoxIo, String> {
        match &self.hop {
            Ok(Hop::Plain) => Ok(io),
            Ok(Hop::Tls(connector, name)) => match connector.connect(name.clone(), io).await {
                Ok(tls) => Ok(Box::new(tls)),
                Err(e) => Err(format!("TLS to the app failed: {e}")),
            },
            Ok(Hop::Broken) => Err("target_tls cannot be used".into()),
            Err(e) => Err(format!("target_tls: {e}")),
        }
    }

    pub async fn run(&self, io: BoxIo) -> Result<(), String> {
        let io = self.secure(io).await?;
        let (mut tx, conn) = hyper::client::conn::http1::Builder::new()
            .max_header_size(MAX_HEAD)
            .max_headers(MAX_HEADERS)
            .handshake(TokioIo::new(io))
            .await
            .map_err(|e| no_http(&e))?;
        let send = tx.send_request(self.request());
        tokio::pin!(conn, send);
        let answer = tokio::select! {
            biased;
            r = &mut send => r,
            r = &mut conn => match r {
                Ok(()) => send.await,
                Err(e) => return Err(no_http(&e)),
            },
        };
        let answer = answer.map_err(|e| no_http(&e))?;
        verdict(answer.status())
    }
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
