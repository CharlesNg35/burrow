use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

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

impl Url {
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

async fn read_more(s: &mut Box<dyn Io>, buf: &mut Vec<u8>, url: &Url) -> Result<usize, String> {
    let mut chunk = [0u8; 16 * 1024];
    match tokio::time::timeout(READ_TIMEOUT, s.read(&mut chunk)).await {
        Err(_) => Err(format!("{} stopped sending data", url.host)),
        Ok(Ok(n)) => {
            buf.extend_from_slice(&chunk[..n]);
            Ok(n)
        }
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(0),
        Ok(Err(e)) => Err(format!("reading from {} failed: {e}", url.host)),
    }
}

struct Head {
    code: u16,
    reason: String,
    length: Option<usize>,
    chunked: bool,
    location: Option<String>,
    size: usize,
}

fn parse_head(buf: &[u8]) -> Result<Option<Head>, String> {
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut resp = httparse::Response::new(&mut headers);
    let size = match resp.parse(buf) {
        Ok(httparse::Status::Complete(n)) => n,
        Ok(httparse::Status::Partial) => return Ok(None),
        Err(e) => return Err(format!("invalid HTTP response: {e}")),
    };
    let header = |name: &str| {
        resp.headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| String::from_utf8_lossy(h.value).trim().to_string())
    };
    let length = match header("content-length") {
        Some(v) => Some(
            v.parse::<usize>()
                .map_err(|_| "invalid Content-Length".to_string())?,
        ),
        None => None,
    };
    Ok(Some(Head {
        code: resp.code.unwrap_or(0),
        reason: resp.reason.unwrap_or("").to_string(),
        length,
        chunked: header("transfer-encoding")
            .is_some_and(|v| v.to_ascii_lowercase().contains("chunked")),
        location: header("location"),
        size,
    }))
}

fn chunk_size(data: &[u8]) -> Result<Option<(usize, usize)>, String> {
    let bad = || "invalid chunked response".to_string();
    let Some(line) = data.windows(2).position(|w| w == b"\r\n") else {
        return Ok(None);
    };
    let size = std::str::from_utf8(&data[..line]).map_err(|_| bad())?;
    let size = size.split(';').next().unwrap_or("").trim();
    let size = u64::from_str_radix(size, 16)
        .ok()
        .filter(|n| *n <= u64::from(u32::MAX))
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(bad)?;
    Ok(Some((line + 2, size)))
}

fn chunk_end(start: usize, size: usize, len: usize) -> Option<usize> {
    start
        .checked_add(size)
        .and_then(|e| e.checked_add(2))
        .filter(|e| *e <= len)
}

pub fn dechunk(mut data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(data.len());
    loop {
        let (start, size) =
            chunk_size(data)?.ok_or_else(|| "invalid chunked response".to_string())?;
        if size == 0 {
            return Ok(out);
        }
        let end = chunk_end(start, size, data.len())
            .filter(|e| &data[e - 2..*e] == b"\r\n")
            .ok_or_else(|| "the download was cut short".to_string())?;
        out.extend_from_slice(&data[start..end - 2]);
        data = &data[end..];
    }
}

fn chunked_complete(mut data: &[u8]) -> Result<bool, String> {
    loop {
        let Some((start, size)) = chunk_size(data)? else {
            return Ok(false);
        };
        if size == 0 {
            let rest = &data[start..];
            return Ok(rest.starts_with(b"\r\n") || rest.windows(4).any(|w| w == b"\r\n\r\n"));
        }
        let Some(end) = chunk_end(start, size, data.len()) else {
            return Ok(false);
        };
        data = &data[end..];
    }
}

async fn fetch_once(
    url: &Url,
    proxy: Option<&ProxyConfig>,
    limit: usize,
) -> Result<(Head, Vec<u8>), String> {
    let mut s = open(url, proxy).await?;
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: burrow/{}\r\nAccept: */*\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n",
        url.path,
        url.host_header(),
        env!("CARGO_PKG_VERSION")
    );
    s.write_all(request.as_bytes())
        .await
        .map_err(|e| format!("sending the request to {} failed: {e}", url.host))?;
    let mut buf = Vec::new();
    let head = loop {
        if let Some(h) = parse_head(&buf)? {
            break h;
        }
        if buf.len() > MAX_HEAD {
            return Err(format!(
                "{} sent a response header that is too large",
                url.host
            ));
        }
        if read_more(&mut s, &mut buf, url).await? == 0 {
            return Err(format!(
                "{} closed the connection without an answer",
                url.host
            ));
        }
    };
    if head.code != 200 {
        return Ok((head, Vec::new()));
    }
    let mut body = buf.split_off(head.size);
    loop {
        let complete = if head.chunked {
            chunked_complete(&body)?
        } else {
            head.length.is_some_and(|n| body.len() >= n)
        };
        if complete {
            break;
        }
        if body.len() > limit + MAX_HEAD {
            return Err(format!("{url} is larger than expected"));
        }
        if read_more(&mut s, &mut body, url).await? == 0 {
            break;
        }
    }
    let body = match head.length {
        _ if head.chunked => dechunk(&body)?,
        Some(n) if body.len() < n => return Err(format!("the download of {url} was cut short")),
        Some(n) => {
            body.truncate(n);
            body
        }
        None => body,
    };
    if body.len() > limit {
        return Err(format!("{url} is larger than expected"));
    }
    Ok((head, body))
}

pub async fn get(
    url: &str,
    limit: usize,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Fetched, String> {
    let mut url = Url::parse(url)?;
    for _ in 0..=MAX_REDIRECTS {
        let proxy = proxy_from_env(&url, env)?;
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

    #[test]
    fn decodes_chunked_bodies() {
        assert_eq!(
            dechunk(b"3\r\nabc\r\n2;x=1\r\nde\r\n0\r\n\r\n").unwrap(),
            b"abcde"
        );
        assert!(dechunk(b"3\r\nabc\r\n").is_err());
        assert!(dechunk(b"5\r\nabc").is_err());
        assert!(dechunk(b"zz\r\n").is_err());
        assert!(dechunk(b"ffffffffffffffff\r\nabc\r\n0\r\n\r\n").is_err());
        assert!(chunked_complete(b"3\r\nabc\r\n0\r\n\r\n").unwrap());
        assert!(chunked_complete(b"3\r\nabc\r\n0\r\nX-Trailer: 1\r\n\r\n").unwrap());
        assert!(!chunked_complete(b"3\r\nabc\r\n0\r\n").unwrap());
        assert!(!chunked_complete(b"5\r\nabc").unwrap());
        assert!(chunked_complete(b"ffffffffffffffff\r\nabc").is_err());
        assert!(dechunk(b"100000000\r\nabc\r\n0\r\n\r\n").is_err());
    }

    #[test]
    fn reads_response_heads() {
        assert!(
            parse_head(b"HTTP/1.1 200 OK\r\nContent-Le")
                .unwrap()
                .is_none()
        );
        let h = parse_head(b"HTTP/1.1 302 Found\r\nLocation: /x\r\nContent-Length: 0\r\n\r\nrest")
            .unwrap()
            .unwrap();
        assert_eq!(
            (h.code, h.location.as_deref(), h.length, h.size),
            (302, Some("/x"), Some(0), 55)
        );
        assert!(parse_head(b"HTTP/1.1 200 OK\r\nContent-Length: x\r\n\r\n").is_err());
        assert!(parse_head(b"garbage\r\n\r\n").is_err());
    }
}
