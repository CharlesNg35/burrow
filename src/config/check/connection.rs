use std::collections::BTreeMap;
use std::path::Path;

use super::Spans;
use crate::diagnostics::Diagnostic;
use crate::proxy::ProxyConfig;
use crate::transport::endpoint::Endpoint;

const RESERVED_HEADERS: &[&str] = &[
    "host",
    "upgrade",
    "connection",
    "content-length",
    "transfer-encoding",
];

fn check_headers(
    given: &BTreeMap<String, String>,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    for (name, value) in given {
        let lower = name.to_ascii_lowercase();
        let reserved =
            RESERVED_HEADERS.contains(&lower.as_str()) || lower.starts_with("sec-websocket-");
        let valid_name = http::HeaderName::try_from(name.as_str()).is_ok();
        if reserved || !valid_name {
            out.push(
                Diagnostic::error("invalid-header", format!("header \"{name}\" cannot be set"))
                    .at(spans.key(&["headers", name]))
                    .label(if reserved { "set by burrow itself" } else { "not a valid header name" })
                    .help("use custom headers such as an access token, for example CF-Access-Client-Id"),
            );
            continue;
        }
        if http::HeaderValue::try_from(value.as_str()).is_err() {
            out.push(
                Diagnostic::error(
                    "invalid-header",
                    format!("header \"{name}\" has an invalid value"),
                )
                .at(spans.value(&["headers", name]))
                .without_snippet()
                .help("header values cannot contain line breaks or control characters"),
            );
            continue;
        }
        headers.push((name.clone(), value.clone()));
    }
    headers
}

pub struct Dial<'a> {
    pub server: &'a str,
    pub headers: &'a BTreeMap<String, String>,
    pub ca: Option<&'a str>,
    pub proxy: Option<&'a str>,
}

pub struct Connection {
    pub endpoint: Option<Endpoint>,
    pub headers: Vec<(String, String)>,
    pub ca_pem: Option<Vec<u8>>,
    pub proxy: Option<ProxyConfig>,
}

pub fn check_connection(
    given: Dial<'_>,
    spans: &Spans,
    base_dir: &Path,
    out: &mut Vec<Diagnostic>,
) -> Connection {
    let endpoint = match Endpoint::parse(given.server) {
        Ok(e) => Some(e),
        Err(e) => {
            out.push(
                Diagnostic::error("invalid-address", format!("invalid server address: {e}"))
                    .at(spans.value(&["server"]))
                    .label("expected a host, host:port or wss://host/path")
                    .help("use a host name or IP (port 7835 unless you add :PORT), or the WebSocket URL (wss://tunnel.example.com/burrow)"),
            );
            None
        }
    };
    let endpoint = endpoint.filter(|_| spans.has(&["server"]));
    let websocket = endpoint.as_ref().is_some_and(Endpoint::is_websocket);
    let headers = check_headers(given.headers, spans, out);
    if !given.headers.is_empty() && !websocket && endpoint.is_some() {
        out.push(
            Diagnostic::warning(
                "unused-option",
                "\"headers\" are only sent over WebSocket and are ignored for a TCP server address",
            )
            .at(spans.key(&["headers"]))
            .help("remove \"headers\", or use a ws:// or wss:// server address"),
        );
    }
    let tls = matches!(endpoint, Some(Endpoint::WebSocket { tls: true, .. }));
    let ca_pem = match given.ca {
        None => None,
        Some(file) => {
            let path = base_dir.join(file);
            match std::fs::read(&path)
                .map_err(|e| crate::failure::file_reason(&e))
                .and_then(|pem| crate::transport::tls::parse_certs(&pem).map(|_| pem))
            {
                Ok(pem) => {
                    if !tls && endpoint.is_some() {
                        out.push(
                            Diagnostic::warning(
                                "unused-option",
                                "\"ca\" is only used for wss:// server addresses",
                            )
                            .at(spans.key(&["ca"]))
                            .help("remove \"ca\", or use a wss:// server address"),
                        );
                    }
                    Some(pem)
                }
                Err(e) => {
                    out.push(
                        Diagnostic::error(
                            "tls-file",
                            format!("cannot use ca file \"{file}\": {e}"),
                        )
                        .at(spans.value(&["ca"]))
                        .label("not a readable PEM certificate")
                        .help("paths are relative to the config file"),
                    );
                    None
                }
            }
        }
    };
    let proxy = match given.proxy {
        Some(p) => match ProxyConfig::parse(p) {
            Ok(p) => Some(p),
            Err(e) => {
                out.push(
                    Diagnostic::error("invalid-proxy", format!("invalid proxy: {e}"))
                        .at(spans.value(&["proxy"]))
                        .without_snippet()
                        .help("use http://host:port or socks5://[user:pass@]host:port"),
                );
                None
            }
        },
        None => None,
    };
    Connection {
        endpoint,
        headers,
        ca_pem,
        proxy,
    }
}
