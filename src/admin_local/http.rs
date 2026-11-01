use burrow_admin_proto::local::{API_PATH, RUN_PREFIX};

pub const MAX_HEAD: usize = 64 * 1024;

pub const SECURITY_HEADERS: [(&str, &str); 6] = [
    (
        "Content-Security-Policy",
        "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' data:; connect-src 'self'; font-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
    ),
    ("X-Content-Type-Options", "nosniff"),
    ("Referrer-Policy", "no-referrer"),
    ("Cross-Origin-Opener-Policy", "same-origin"),
    ("Cross-Origin-Resource-Policy", "same-origin"),
    ("X-Frame-Options", "DENY"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reject {
    pub code: u16,
    pub reason: &'static str,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Head {
    pub method: String,
    pub path: String,
    pub host: Option<String>,
    pub origin: Option<String>,
    pub fetch_site: Option<String>,
    pub fetch_mode: Option<String>,
    pub fetch_dest: Option<String>,
    pub upgrade: bool,
    pub gzip: bool,
}

pub fn head_of<B>(req: &::http::Request<B>) -> Result<Head, Reject> {
    let h = req.headers();
    let one = |name: &str| -> Result<Option<String>, Reject> {
        let mut all = h.get_all(name).iter();
        let first = all.next();
        if all.next().is_some() {
            return Err(Reject {
                code: 400,
                reason: "repeated header",
            });
        }
        Ok(first.map(|v| String::from_utf8_lossy(v.as_bytes()).trim().to_string()))
    };
    let text = |v: &::http::HeaderValue| String::from_utf8_lossy(v.as_bytes()).trim().to_string();
    Ok(Head {
        method: req.method().as_str().to_string(),
        path: req.uri().to_string(),
        host: one(::http::header::HOST.as_str())?,
        origin: one("origin")?,
        fetch_site: one("sec-fetch-site")?,
        fetch_mode: one("sec-fetch-mode")?,
        fetch_dest: one("sec-fetch-dest")?,
        upgrade: h
            .get_all("upgrade")
            .iter()
            .any(|v| text(v).eq_ignore_ascii_case(crate::wire::transport::ws::UPGRADE_TOKEN)),
        gzip: h
            .get_all("accept-encoding")
            .iter()
            .any(|v| text(v).split(',').any(|e| e.trim().starts_with("gzip"))),
    })
}

fn allowed_hosts(port: u16) -> [String; 3] {
    [
        format!("127.0.0.1:{port}"),
        format!("localhost:{port}"),
        format!("[::1]:{port}"),
    ]
}

pub fn run_prefix() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("operating system random source is available");
    let hex = hex::encode(bytes);
    format!("{RUN_PREFIX}{hex}")
}

pub fn is_file(path: &str) -> bool {
    path.strip_prefix('/')
        .is_some_and(|name| !name.contains('/') && name.contains('.'))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Page(String),
    Asset(String),
    Api,
    Alive,
    Redirect(String),
    NotFound,
    Reject,
}

fn climbs(path: &str) -> bool {
    percent_encoding::percent_decode_str(path)
        .decode_utf8_lossy()
        .split(['/', '\\'])
        .any(|segment| segment == "..")
}

fn without_run(path: &str) -> &str {
    match path.strip_prefix(RUN_PREFIX) {
        Some(rest) => rest.find('/').map_or("/", |i| &rest[i..]),
        None => path,
    }
}

pub fn route(target: &str, prefix: &str, upgrade: bool) -> Decision {
    let path = target.split(['?', '#']).next().unwrap_or("/");
    let inner = path
        .strip_prefix(prefix)
        .filter(|rest| rest.starts_with('/'));
    if inner == Some(API_PATH) {
        return Decision::Api;
    }
    if !path.starts_with('/') || climbs(path) {
        return Decision::Reject;
    }
    if let Some(inner) = inner {
        return if is_file(inner) {
            Decision::Asset(inner.to_string())
        } else {
            Decision::Page(inner.to_string())
        };
    }
    let rest = without_run(path);
    let last = rest.rsplit('/').next().unwrap_or("");
    if rest == API_PATH || (upgrade && last == API_PATH.trim_start_matches('/')) || is_file(rest) {
        return Decision::NotFound;
    }
    let query = target
        .split_once('?')
        .map(|(_, q)| format!("?{}", q.split('#').next().unwrap_or("")))
        .unwrap_or_default();
    Decision::Redirect(format!("{prefix}{rest}{query}"))
}

pub fn check(head: &Head, port: u16, prefix: &str) -> Result<Decision, Reject> {
    let forbidden = |reason| Reject { code: 403, reason };
    let host = head.host.as_deref().ok_or(Reject {
        code: 400,
        reason: "no Host header",
    })?;
    if !allowed_hosts(port)
        .iter()
        .any(|h| h.eq_ignore_ascii_case(host))
    {
        return Err(forbidden("this page is served for 127.0.0.1 only"));
    }
    let opened = !head.upgrade
        && head.fetch_mode.as_deref() == Some("navigate")
        && head.fetch_dest.as_deref() == Some("document");
    if let Some(site) = head.fetch_site.as_deref()
        && !matches!(site, "same-origin" | "none")
        && !opened
    {
        return Err(forbidden("requests from other sites are refused"));
    }
    if head.method != "GET" {
        return Err(Reject {
            code: 405,
            reason: "only GET is supported",
        });
    }
    match route(&head.path, prefix, head.upgrade) {
        Decision::Api => {
            if !head.upgrade {
                return Ok(Decision::Alive);
            }
            let expected = format!("http://{host}");
            if head.origin.as_deref() != Some(expected.as_str()) {
                return Err(forbidden("the WebSocket must come from this page"));
            }
            Ok(Decision::Api)
        }
        Decision::Reject => Err(Reject {
            code: 400,
            reason: "bad path",
        }),
        Decision::NotFound => Err(Reject {
            code: 404,
            reason: "not found",
        }),
        other => Ok(other),
    }
}

pub type Body = http_body_util::Full<bytes::Bytes>;

pub fn response(
    code: u16,
    mime: &str,
    extra: &[(&str, String)],
    body: impl Into<bytes::Bytes>,
) -> ::http::Response<Body> {
    let body: bytes::Bytes = body.into();
    let mut b = ::http::Response::builder()
        .status(code)
        .header("Content-Type", mime)
        .header("Content-Length", body.len());
    for (k, v) in SECURITY_HEADERS {
        b = b.header(k, v);
    }
    for (k, v) in extra {
        b = b.header(*k, v.as_str());
    }
    b.header("Connection", "close")
        .body(Body::new(body))
        .unwrap_or_else(|_| ::http::Response::new(Body::default()))
}

#[cfg(test)]
mod tests;
