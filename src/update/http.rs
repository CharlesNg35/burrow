use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::proxy::ProxyConfig;
use crate::transport::proxy::dial_tcp;
use crate::transport::resolve::SystemResolver;
use crate::transport::tls;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_HEAD: usize = 64 * 1024;
const MAX_REDIRECTS: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    pub tls: bool,
    pub host: String,
    pub port: u16,
    pub path: String,
}

fn loopback_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

impl Url {
    fn is_loopback(&self) -> bool {
        loopback_host(&self.host)
    }

    pub fn parse(url: &str) -> Result<Url, String> {
        let (tls, rest) = if let Some(r) = url.strip_prefix("https://") {
            (true, r)
        } else if let Some(r) = url.strip_prefix("http://") {
            (false, r)
        } else {
            return Err(format!("{url} is not an http:// or https:// URL"));
        };
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let default = if tls { 443 } else { 80 };
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) if !p.contains(']') => (
                h,
                p.parse::<u16>()
                    .map_err(|_| format!("{url} has an invalid port"))?,
            ),
            _ => (authority, default),
        };
        if host.is_empty() || host.contains('@') {
            return Err(format!("{url} has no valid host"));
        }
        Ok(Url {
            tls,
            host: host.to_string(),
            port,
            path: path.to_string(),
        })
    }

    fn join(&self, location: &str) -> Result<Url, String> {
        if location.starts_with("http://") || location.starts_with("https://") {
            return Url::parse(location);
        }
        if !location.starts_with('/') {
            return Err(format!("unsupported redirect to {location}"));
        }
        Ok(Url {
            path: location.to_string(),
            ..self.clone()
        })
    }

    fn authority(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    fn host_header(&self) -> String {
        if self.port == if self.tls { 443 } else { 80 } {
            self.host.clone()
        } else {
            self.authority()
        }
    }
}

impl std::fmt::Display for Url {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let scheme = if self.tls { "https" } else { "http" };
        write!(f, "{scheme}://{}{}", self.host_header(), self.path)
    }
}

fn no_proxy(host: &str, list: &str) -> bool {
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    list.split(',').map(str::trim).any(|p| {
        let p = p.trim_start_matches('.').to_ascii_lowercase();
        p == "*" || (!p.is_empty() && (host == p || host.ends_with(&format!(".{p}"))))
    })
}

pub fn proxy_from_env(
    url: &Url,
    get: impl Fn(&str) -> Option<String>,
) -> Result<Option<ProxyConfig>, String> {
    let pick = |names: &[&str]| {
        names
            .iter()
            .find_map(|n| get(n).filter(|v| !v.trim().is_empty()))
    };
    if let Some(list) = pick(&["NO_PROXY", "no_proxy"])
        && no_proxy(&url.host, &list)
    {
        return Ok(None);
    }
    let names: &[&str] = if url.tls {
        &["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]
    } else {
        &["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"]
    };
    let found = names
        .iter()
        .find_map(|n| get(n).filter(|v| !v.trim().is_empty()).map(|v| (*n, v)));
    match found {
        Some((name, v)) => ProxyConfig::parse(v.trim())
            .map(Some)
            .map_err(|e| format!("{name} is not a valid proxy: {e}")),
        None => Ok(None),
    }
}

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

pub enum Fetched {
    Body(Vec<u8>),
    NotFound,
}

async fn open(url: &Url, proxy: Option<&ProxyConfig>) -> Result<Box<dyn Io>, String> {
    let via = proxy
        .map(|p| format!(" through proxy {p:?}"))
        .unwrap_or_default();
    let tcp = dial_tcp(&url.authority(), proxy, CONNECT_TIMEOUT, &SystemResolver)
        .await
        .map_err(|e| format!("cannot connect to {}{via}: {e}", url.authority()))?;
    if !url.tls {
        return Ok(Box::new(tcp));
    }
    let connector = tls::connector(None)?;
    let name = tls::server_name(url.host.trim_matches(['[', ']']))?;
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, connector.connect(name, tcp))
        .await
        .map_err(|_| format!("TLS handshake with {} timed out", url.host))?
        .map_err(|e| format!("TLS handshake with {} failed: {e}", url.host))?;
    Ok(Box::new(stream))
}

struct Head {
    code: u16,
    reason: String,
    location: Option<String>,
}

fn failed(e: &hyper::Error, url: &Url, at_head: bool) -> String {
    if e.is_parse_too_large() {
        format!("{} sent a response header that is too large", url.host)
    } else if e.is_incomplete_message() && at_head {
        format!("{} closed the connection without an answer", url.host)
    } else if !at_head {
        format!("the download of {url} was cut short: {e}")
    } else if e.is_parse() {
        format!("invalid HTTP response: {e}")
    } else {
        format!("reading from {} failed: {e}", url.host)
    }
}

async fn fetch_once(
    url: &Url,
    proxy: Option<&ProxyConfig>,
    limit: usize,
) -> Result<(Head, Vec<u8>), String> {
    use http_body_util::BodyExt;
    let s = open(url, proxy).await?;
    let (mut sender, conn) = hyper::client::conn::http1::Builder::new()
        .title_case_headers(true)
        .max_buf_size(MAX_HEAD)
        .handshake::<_, http_body_util::Empty<bytes::Bytes>>(hyper_util::rt::TokioIo::new(s))
        .await
        .map_err(|e| format!("sending the request to {} failed: {e}", url.host))?;
    let driver = tokio::spawn(conn);
    let result = async {
        let request = ::http::Request::get(url.path.as_str())
            .header("Host", url.host_header())
            .header(
                "User-Agent",
                format!("burrow/{}", env!("CARGO_PKG_VERSION")),
            )
            .header("Accept", "*/*")
            .header("Accept-Encoding", "identity")
            .header("Connection", "close")
            .body(http_body_util::Empty::new())
            .map_err(|e| format!("cannot request {url}: {e}"))?;
        let response = tokio::time::timeout(READ_TIMEOUT, sender.send_request(request))
            .await
            .map_err(|_| format!("{} stopped sending data", url.host))?
            .map_err(|e| failed(&e, url, true))?;
        let status = response.status();
        let head = Head {
            code: status.as_u16(),
            reason: response
                .extensions()
                .get::<hyper::ext::ReasonPhrase>()
                .map(|r| String::from_utf8_lossy(r.as_bytes()).into_owned())
                .or_else(|| status.canonical_reason().map(str::to_string))
                .unwrap_or_default(),
            location: response
                .headers()
                .get("location")
                .map(|v| String::from_utf8_lossy(v.as_bytes()).trim().to_string()),
        };
        if head.code != 200 {
            return Ok((head, Vec::new()));
        }
        let mut body = response.into_body();
        let mut out = Vec::new();
        loop {
            let frame = tokio::time::timeout(READ_TIMEOUT, body.frame())
                .await
                .map_err(|_| format!("{} stopped sending data", url.host))?;
            match frame {
                None => break,
                Some(Err(e)) => return Err(failed(&e, url, false)),
                Some(Ok(f)) => {
                    if let Ok(data) = f.into_data() {
                        out.extend_from_slice(&data);
                        if out.len() > limit {
                            return Err(format!("{url} is larger than expected"));
                        }
                    }
                }
            }
        }
        Ok((head, out))
    }
    .await;
    driver.abort();
    result
}

pub async fn get(
    url: &str,
    limit: usize,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Fetched, String> {
    let mut url = Url::parse(url)?;
    for _ in 0..=MAX_REDIRECTS {
        if !url.tls && !url.is_loopback() {
            return Err(format!(
                "{url} is plain http, where anyone on the way could swap the download; use https:// (http:// is accepted only for this machine)"
            ));
        }
        let proxy = proxy_from_env(&url, env)?;
        if !url.tls
            && let Some(p) = &proxy
            && !loopback_host(p.addr.rsplit_once(':').map_or(p.addr.as_str(), |(h, _)| h))
        {
            return Err(format!(
                "{url} is plain http, and the proxy {} would carry it over the network; use https://",
                p.addr
            ));
        }
        let (head, body) = fetch_once(&url, proxy.as_ref(), limit).await?;
        match head.code {
            200 => return Ok(Fetched::Body(body)),
            404 => return Ok(Fetched::NotFound),
            301 | 302 | 303 | 307 | 308 => {
                let to = head
                    .location
                    .ok_or_else(|| format!("{url} redirected without a Location"))?;
                let next = url.join(&to)?;
                if url.tls && !next.tls {
                    return Err(format!("{url} redirected from https to {next}"));
                }
                url = next;
            }
            code => {
                return Err(format!("{url} answered {code} {}", head.reason)
                    .trim_end()
                    .to_string());
            }
        }
    }
    Err(format!("too many redirects from {url}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn plain_http_is_refused_except_for_this_machine() {
        for local in [
            "http://127.0.0.1:1/r",
            "http://[::1]:1/r",
            "http://localhost/r",
        ] {
            assert!(Url::parse(local).unwrap().is_loopback(), "{local}");
        }
        for remote in [
            "http://10.0.0.1/r",
            "http://mirror.example/r",
            "https://[::1]/r",
        ] {
            let url = Url::parse(remote).unwrap();
            assert!(url.tls || !url.is_loopback(), "{remote}");
        }
        let refused = get("http://mirror.example/releases", 1024, &|_| None)
            .await
            .err()
            .unwrap();
        assert!(refused.contains("use https://"), "{refused}");
        let proxied = get("http://127.0.0.1:1/releases", 1024, &|n: &str| {
            (n == "HTTP_PROXY").then(|| "http://proxy.example:3128".to_string())
        })
        .await
        .err()
        .unwrap();
        assert!(proxied.contains("over the network"), "{proxied}");
    }

    #[test]
    fn parses_urls() {
        let u = Url::parse("https://github.com/a/b/releases").unwrap();
        assert_eq!(
            u,
            Url {
                tls: true,
                host: "github.com".into(),
                port: 443,
                path: "/a/b/releases".into()
            }
        );
        assert_eq!(u.to_string(), "https://github.com/a/b/releases");
        let u = Url::parse("http://127.0.0.1:8080").unwrap();
        assert_eq!((u.port, u.path.as_str()), (8080, "/"));
        assert_eq!(u.to_string(), "http://127.0.0.1:8080/");
        let u = Url::parse("http://[::1]:81/x").unwrap();
        assert_eq!((u.host.as_str(), u.port), ("[::1]", 81));
        assert_eq!(Url::parse("https://[::1]/x").unwrap().port, 443);
        assert!(Url::parse("ftp://x").is_err());
        assert!(Url::parse("https://:80/").is_err());
        assert!(Url::parse("https://u@x/").is_err());
        assert!(Url::parse("https://x:99999/").is_err());
    }

    #[test]
    fn follows_relative_and_absolute_redirects() {
        let u = Url::parse("https://github.com/a").unwrap();
        assert_eq!(
            u.join("/b?c=1").unwrap().to_string(),
            "https://github.com/b?c=1"
        );
        assert_eq!(
            u.join("https://objects.example.com/x").unwrap().host,
            "objects.example.com"
        );
        assert!(u.join("relative").is_err());
    }

    #[test]
    fn proxies_come_from_the_environment() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        let https = Url::parse("https://github.com/").unwrap();
        let http = Url::parse("http://mirror.local/").unwrap();
        let p = proxy_from_env(&https, env(&[("HTTPS_PROXY", "http://proxy:3128")])).unwrap();
        assert_eq!(p.unwrap().addr, "proxy:3128");
        assert!(
            proxy_from_env(&http, env(&[("HTTPS_PROXY", "http://proxy:3128")]))
                .unwrap()
                .is_none()
        );
        let p = proxy_from_env(&http, env(&[("all_proxy", "socks5://s:1080")])).unwrap();
        assert_eq!(p.unwrap().addr, "s:1080");
        let both: &[(&str, &str)] = &[("HTTPS_PROXY", "http://p:1"), ("NO_PROXY", ".github.com")];
        assert!(proxy_from_env(&https, env(both)).unwrap().is_none());
        let other: &[(&str, &str)] = &[("HTTPS_PROXY", "http://p:1"), ("NO_PROXY", "hub.com")];
        assert!(proxy_from_env(&https, env(other)).unwrap().is_some());
        assert!(proxy_from_env(&https, env(&[("HTTPS_PROXY", "nope")])).is_err());
        let leaked = proxy_from_env(&https, env(&[("HTTPS_PROXY", "https://bob:S3cret@p:3128")]))
            .unwrap_err();
        assert!(
            leaked.contains("HTTPS_PROXY") && !leaked.contains("S3cret"),
            "{leaked}"
        );
    }

    async fn answer(reply: &'static [u8]) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf).await;
                let _ = s.write_all(reply).await;
                let _ = s.shutdown().await;
            }
        });
        format!("http://127.0.0.1:{port}/x")
    }

    async fn body_of(reply: &'static [u8]) -> Result<Vec<u8>, String> {
        match get(&answer(reply).await, 1 << 20, &|_| None).await? {
            Fetched::Body(b) => Ok(b),
            Fetched::NotFound => Err("not found".into()),
        }
    }

    #[tokio::test]
    async fn decodes_chunked_bodies() {
        let head = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";
        let with = |body: &str| -> &'static [u8] {
            Box::leak(format!("{head}{body}").into_boxed_str()).as_bytes()
        };
        assert_eq!(
            body_of(with("3\r\nabc\r\n2;x=1\r\nde\r\n0\r\n\r\n"))
                .await
                .unwrap(),
            b"abcde"
        );
        assert_eq!(
            body_of(with("3\r\nabc\r\n0\r\nX-Trailer: 1\r\n\r\n"))
                .await
                .unwrap(),
            b"abc"
        );
        for bad in [
            "3\r\nabc\r\n",
            "5\r\nabc",
            "zz\r\n",
            "ffffffffffffffff\r\nabc\r\n0\r\n\r\n",
            "100000000\r\nabc\r\n0\r\n\r\n",
            "3\r\nabc\r\n0\r\n",
        ] {
            assert!(body_of(with(bad)).await.is_err(), "{bad:?}");
        }
    }

    #[tokio::test]
    async fn reads_response_heads() {
        let cut = body_of(b"HTTP/1.1 200 OK\r\nContent-Le").await.unwrap_err();
        assert!(
            cut.contains("without an answer") || cut.contains("invalid"),
            "{cut}"
        );
        assert!(
            body_of(b"HTTP/1.1 200 OK\r\nContent-Length: x\r\n\r\n")
                .await
                .is_err()
        );
        assert!(body_of(b"garbage\r\n\r\n").await.is_err());
        let short = body_of(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nabc")
            .await
            .unwrap_err();
        assert!(short.contains("cut short"), "{short}");
        assert_eq!(
            body_of(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabcdef")
                .await
                .unwrap(),
            b"abc"
        );
        let target = answer(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").await;
        let moved: &'static [u8] = Box::leak(
            format!("HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\n\r\n")
                .into_bytes()
                .into_boxed_slice(),
        );
        assert_eq!(body_of(moved).await.unwrap(), b"ok");
        let missing = body_of(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap_err();
        assert_eq!(missing, "not found");
        let teapot = body_of(b"HTTP/1.1 418 Teapot Here\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap_err();
        assert!(teapot.ends_with("answered 418 Teapot Here"), "{teapot}");
    }

    #[tokio::test]
    async fn a_redirect_loop_stops_at_the_cap() {
        let looped = body_of(b"HTTP/1.1 302 Found\r\nLocation: /x\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap_err();
        assert!(looped.starts_with("too many redirects"), "{looped}");
    }
}
