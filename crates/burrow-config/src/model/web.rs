use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::admin_schema::AdminSchema;

use super::{
    AccessLog, Bind, CertFile, ForwardAuthConfig, LetsEncrypt, RedirectTarget, SiteTls, WebSite,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[admin(label = "Web proxy")]
pub struct WebConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Port or ip:port for HTTPS, like 443; a port alone means every IPv4 and IPv6 address, 0.0.0.0:443 only IPv4"
    )]
    #[admin(
        ty = ip_port(default_ip = "::"),
        label = "HTTPS port",
        help = "The port where websites and TLS services are reached by name, for example 443; a port alone listens on IPv4 and IPv6, 0.0.0.0:443 on IPv4 only",
        none_label = "No HTTPS port"
    )]
    pub https: Option<Bind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Port or ip:port for plain HTTP, like 80: plain-HTTP sites, the redirect to HTTPS and Let's Encrypt's HTTP check; a port alone means every IPv4 and IPv6 address"
    )]
    #[admin(
        ty = ip_port(default_ip = "::"),
        label = "HTTP port",
        help = "The port for plain HTTP, for example 80: sites with HTTPS off, the step from http:// to https:// and Let's Encrypt's check; a port alone listens on IPv4 and IPv6",
        none_label = "No HTTP port"
    )]
    pub http: Option<Bind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "What an HTTPS site does on web.http: true sends the visitor to the same address on https:// (308), false answers plain HTTP too; sites with tls: off default to false"
    )]
    #[admin(
        label = "Send http:// to https://",
        help = "For every HTTPS website: send a visitor who types http://example.com to https://example.com, or answer plain HTTP too; a website can change it",
        none_label = "Yes",
        advanced
    )]
    pub https_redirect: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "true (the default) lets browsers use HTTP/2 on web.https; false keeps HTTP/1.1 only"
    )]
    #[admin(
        label = "HTTP/2",
        help = "Let browsers use HTTP/2 on the HTTPS port, faster for pages with many parts; turn it off only for a client that breaks with it",
        none_label = "On",
        advanced
    )]
    pub http2: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "The login service sites use with forward_auth: true, {service, path, copy_headers}"
    )]
    #[admin(
        label = "Login service",
        help = "The single sign-on service websites can share to check their visitors, for example Authelia",
        none_label = "None",
        advanced
    )]
    pub forward_auth: Option<ForwardAuthConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Default for every site: requests per second per visitor address (IPv6 per /64), bursts of twice that, 429 above"
    )]
    #[schemars(range(min = 1))]
    #[admin(
        ty = integer(unit = "requests per second"),
        help = "For every website: how many requests per second one visitor may send, for example 20; a website can change it",
        none_label = "Off",
        advanced
    )]
    pub rate_limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Automatic certificates from Let's Encrypt (or another ACME server); writing it accepts Let's Encrypt's terms, for example {email: ops@example.com}"
    )]
    #[admin(
        label = "Let's Encrypt",
        help = "Automatic certificates for every website name no file covers; turning it on accepts Let's Encrypt's terms, for example with the email ops@example.com",
        none_label = "Off"
    )]
    pub letsencrypt: Option<LetsEncrypt>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(
        description = "Shared certificate files, each for the names its hosts lists, for example [{hosts: [\"*.example.com\"], cert: certs/wild.pem, key: certs/wild.key}]"
    )]
    #[admin(
        ty = list(object(CertFile)),
        label = "Certificate files",
        help = "Your own certificate files, each used for exactly the names it lists, for example *.example.com with certs/wild.pem and certs/wild.key",
        empty_label = "None",
        advanced
    )]
    pub certificates: Vec<CertFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Seconds a visitor may take to send a request's headers, also the keep-alive idle time; 10 when not set"
    )]
    #[schemars(range(min = 1))]
    #[admin(
        label = "Request header timeout",
        help = "How long a visitor may take to send a request's headers, and how long an idle kept-alive connection stays open, for example 10 seconds",
        none_label = "10 seconds",
        advanced
    )]
    pub request_header_timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Seconds without traffic before a connection is closed, WebSockets excepted; 60 when not set"
    )]
    #[schemars(range(min = 1))]
    #[admin(
        label = "Idle timeout",
        help = "A connection that moves no data for this long is closed, for example 60 seconds; WebSockets stay open",
        none_label = "60 seconds",
        advanced
    )]
    pub idle_timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(
        description = "Whole names, or a name and a path, with no site of their own that only redirect, for example {old.example.com: https://example.com}; temporary unless permanent: true"
    )]
    #[admin(
        ty = map(key = text, value = object(RedirectTarget)),
        label = "Names that only redirect",
        help = "A name with no site of its own that sends visitors elsewhere, path and query kept, for example old.example.com to https://example.com, or example.com/shop to https://shop.example.net/",
        empty_label = "None",
        advanced
    )]
    pub redirect_names: BTreeMap<String, RedirectTarget>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(
        description = "Address ranges of HTTP proxies in front of burrow whose X-Forwarded-For and X-Forwarded-Proto are believed, for example [173.245.48.0/20]"
    )]
    #[admin(
        ty = list(cidr),
        help = "HTTP proxies in front of burrow, like Cloudflare, whose X-Forwarded-For tells the visitor's address, for example 173.245.48.0/20; behind cloudflared, its own address like 127.0.0.1/32",
        empty_label = "None",
        advanced
    )]
    pub trusted_proxies: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(
        description = "Address ranges of TCP load balancers that start each connection with a PROXY header (v1 or v2); they must send it, nobody else may, for example [10.0.0.5/32]"
    )]
    #[admin(
        ty = list(cidr),
        label = "PROXY headers from",
        help = "Load balancers in front of burrow that send a PROXY header with the visitor's address, for example 10.0.0.5/32; they must send it, and nobody else may",
        empty_label = "None",
        advanced,
        locked
    )]
    pub proxy_protocol_from: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Write one line per request to a file, or {file, format: common|json}, for example logs/access.log"
    )]
    #[admin(
        help = "One line per request, written to a file, for example logs/access.log, in the common format or as JSON",
        none_label = "Off",
        advanced,
        locked
    )]
    pub access_log: Option<AccessLog>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "true gives each request an X-Request-Id, sent to the app, returned to the visitor and written to the access log; false when not set"
    )]
    #[admin(
        label = "Request ids",
        help = "Give each request an X-Request-Id that the app, the visitor and the access log all see; one from a trusted proxy is kept",
        none_label = "Off",
        advanced
    )]
    pub request_id: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Default for every site: true sends Strict-Transport-Security for a year on HTTPS answers; false when not set"
    )]
    #[admin(
        label = "HSTS",
        help = "For every HTTPS website: tell browsers to use only HTTPS for a year; a website can change it",
        none_label = "Off",
        advanced
    )]
    pub hsts: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Default for every site: true gzips text answers when the browser accepts it; false when not set"
    )]
    #[admin(
        help = "For every website: gzip text answers like HTML, CSS and JSON when the browser accepts it; a website can change it",
        none_label = "Off",
        advanced
    )]
    pub compress: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Default for every site: the biggest request body, like 100MB; above it the visitor gets 413; unlimited when not set"
    )]
    #[admin(
        label = "Biggest upload",
        help = "For every website: the biggest request body, for example 100MB; bigger uploads get 413; a website can change it",
        none_label = "Unlimited",
        advanced
    )]
    pub max_body_size: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Default for every site: seconds the app may take to start its answer, 504 after; 60 when not set"
    )]
    #[schemars(range(min = 1))]
    #[admin(
        label = "App answer timeout",
        help = "For every website: how long the app may take to start its answer before the visitor gets 504, for example 60 seconds; a website can change it",
        none_label = "60 seconds",
        advanced
    )]
    pub response_header_timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(
        description = "Default for every site: headers set on requests to the app, Name: value; an empty value removes the header"
    )]
    #[admin(
        ty = map(key = text, value = text, value_help = "The header's value, for example DENY; empty removes the header"),
        help = "For every website: headers set on requests to the app, for example X-Frame-Options: DENY; an empty value removes one",
        empty_label = "None",
        advanced
    )]
    pub request_headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(
        description = "Default for every site: headers set on answers to visitors, Name: value; an empty value removes the header"
    )]
    #[admin(
        ty = map(key = text, value = text, value_help = "The header's value, for example DENY; empty removes the header"),
        label = "Answer headers",
        help = "For every website: headers set on answers, for example X-Content-Type-Options: nosniff; an empty value like Server: \"\" removes one",
        empty_label = "None",
        advanced
    )]
    pub response_headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Default for every site: a folder with any of 404.html, 413.html, 429.html, 502.html, 503.html, 504.html and maintenance.html, relative to the config file"
    )]
    #[admin(
        ty = path(must_exist, relative_to = "config"),
        help = "For every website: a folder with your own pages, for example pages/, holding any of 404.html, 413.html, 429.html, 502.html, 503.html, 504.html and maintenance.html; the rest stay built in",
        none_label = "Built in",
        advanced,
        locked
    )]
    pub error_pages: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
#[admin(semantic = union(host_name, list(host_name), object(WebSite)))]
pub enum ServiceWeb {
    #[admin(label = "One name")]
    Host(String),
    #[admin(label = "Several names")]
    Hosts(Vec<String>),
    #[admin]
    Site(Box<WebSite>),
}

impl ServiceWeb {
    pub fn hosts(&self) -> &[String] {
        match self {
            ServiceWeb::Host(h) => std::slice::from_ref(h),
            ServiceWeb::Hosts(h) => h,
            ServiceWeb::Site(s) => &s.hosts,
        }
    }

    pub fn tls(&self) -> Option<&SiteTls> {
        match self {
            ServiceWeb::Site(s) => s.tls.as_ref(),
            _ => None,
        }
    }

    pub fn https_redirect(&self) -> Option<bool> {
        match self {
            ServiceWeb::Site(s) => s.https_redirect,
            _ => None,
        }
    }
}
