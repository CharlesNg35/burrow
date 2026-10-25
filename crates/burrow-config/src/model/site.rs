use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::admin_schema::AdminSchema;

use super::{RedirectTarget, TargetTls, TlsFiles};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[admin(label = "Website settings")]
pub struct WebSite {
    #[schemars(
        description = "Names that lead to this service: DNS names, *. wildcards of one label, or IP addresses, for example [nas.example.com]"
    )]
    #[admin(
        ty = list(host_name),
        label = "Host names",
        help = "Names that lead to this service, for example nas.example.com, *.apps.example.com or 203.0.113.10"
    )]
    pub hosts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "auto (a certificate for every name, the default with web.https), passthrough (the app does its own HTTPS; burrow only reads the name), off (plain HTTP) or {cert, key} (this site's own certificate file)"
    )]
    #[admin(
        label = "HTTPS",
        help = "Who does HTTPS for this website: burrow with an automatic certificate, the app itself (burrow only reads the name), nobody (plain HTTP), or burrow with your own certificate file",
        none_label = "Automatic"
    )]
    pub tls: Option<SiteTls>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "true sends http:// visitors of this site to https:// (308), false answers plain HTTP too; from web.https_redirect when not set"
    )]
    #[admin(
        label = "Send http:// to https://",
        help = "Send a visitor who types http://nas.example.com to https://nas.example.com, or answer plain HTTP too",
        none_label = "As the web proxy says",
        visible_if(all(ne(tls, "passthrough"), ne(tls, "off")))
    )]
    pub https_redirect: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "How Let's Encrypt checks this site's names: tls (on web.https 443), http (on web.http 80) or dns; automatic when not set: TLS first, then HTTP"
    )]
    #[admin(
        label = "Let's Encrypt check",
        help = "How Let's Encrypt checks that the names are yours: over HTTP for a site behind Cloudflare's proxy, for example; automatic tries TLS on port 443 first, then HTTP on port 80",
        none_label = "Automatic",
        advanced,
        visible_if(all(ne(tls, "passthrough"), ne(tls, "off")))
    )]
    pub challenge: Option<ChallengeKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(
        description = "Only these paths on those names, each with everything below it, for example [/api]; the longest matching path wins; all paths when not set"
    )]
    #[admin(
        ty = list(url_path),
        help = "Only these paths on the names lead here, each with everything below it, for example /api for /api and /api/users but not /apix; the longest match wins",
        empty_label = "All paths",
        visible_if(ne(tls, "passthrough"))
    )]
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "What the app sees in place of the matched path: paths: [/api] with app_path: /v1 sends /api/users as /v1/users; / removes the prefix"
    )]
    #[admin(
        ty = url_path,
        label = "Path on the app",
        help = "What the app sees in place of the matched path, for example /v1 sends /api/users as /v1/users, and / removes /api",
        none_label = "The same path",
        advanced,
        visible_if(all(ne(tls, "passthrough"), len_ge(paths, 1)))
    )]
    pub app_path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(
        description = "Names that redirect permanently to the first host, path and query kept, for example [www.example.com]"
    )]
    #[admin(
        ty = list(host_name),
        help = "Names that send visitors to the first host name for good, path kept, for example www.example.com",
        empty_label = "None",
        advanced,
        visible_if(ne(tls, "passthrough"))
    )]
    pub redirect_from: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(
        description = "Paths on this site that redirect, for example {/old: /new} (temporary) or {/old: {to: /new, permanent: true}}"
    )]
    #[admin(
        ty = map(key = url_path, value = object(RedirectTarget)),
        help = "Paths that send visitors elsewhere, the rest of the path and the query kept, for example /old to /new or to https://example.net/new",
        empty_label = "None",
        advanced,
        visible_if(ne(tls, "passthrough"))
    )]
    pub redirects: BTreeMap<String, RedirectTarget>,
    #[serde(default, skip_serializing_if = "super::is_false")]
    #[schemars(
        description = "true answers every request with the maintenance page (503 with Retry-After)"
    )]
    #[admin(
        help = "Answer every visitor with the maintenance page instead of the app",
        visible_if(ne(tls, "passthrough"))
    )]
    pub maintenance: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "The app speaks HTTPS: true checks its certificate against the public CAs, {ca: file} against your CA, {insecure_skip_verify: true} not at all"
    )]
    #[admin(
        label = "HTTPS to the app",
        help = "For an app that speaks HTTPS itself: check its certificate against the public CAs or your own CA, or not at all for a self-made one like Proxmox's",
        none_label = "Off: plain HTTP to the app",
        advanced,
        visible_if(ne(tls, "passthrough"))
    )]
    pub target_tls: Option<TargetTls>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "true sends Strict-Transport-Security for a year on this site's HTTPS answers; from web.hsts when not set"
    )]
    #[admin(
        label = "HSTS",
        help = "Tell browsers to use only HTTPS for this site for a year",
        none_label = "As the web proxy says",
        advanced,
        visible_if(all(ne(tls, "passthrough"), ne(tls, "off")))
    )]
    pub hsts: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "true gzips text answers when the browser accepts it; from web.compress when not set"
    )]
    #[admin(
        help = "Gzip text answers like HTML, CSS and JSON when the browser accepts it",
        none_label = "As the web proxy says",
        advanced,
        visible_if(ne(tls, "passthrough"))
    )]
    pub compress: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "The biggest request body, like 8GB; above it the visitor gets 413; from web.max_body_size when not set"
    )]
    #[admin(
        label = "Biggest upload",
        help = "The biggest request body, for example 8GB for ISO uploads; bigger uploads get 413",
        none_label = "As the web proxy says",
        advanced,
        visible_if(ne(tls, "passthrough"))
    )]
    pub max_body_size: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Seconds the app may take to start its answer, 504 after; from web.response_header_timeout_secs when not set"
    )]
    #[schemars(range(min = 1))]
    #[admin(
        label = "App answer timeout",
        help = "How long the app may take to start its answer before the visitor gets 504, for example 300 seconds for slow reports",
        none_label = "As the web proxy says",
        advanced,
        visible_if(ne(tls, "passthrough"))
    )]
    pub response_header_timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(
        description = "Headers set on requests to the app, Name: value, also Host; an empty value removes the header; added to web.request_headers"
    )]
    #[admin(
        ty = map(key = text, value = text),
        help = "Headers set on requests to the app, for example Host: app.internal for an app that wants its own name; an empty value removes one",
        empty_label = "As the web proxy says",
        advanced,
        visible_if(ne(tls, "passthrough"))
    )]
    pub request_headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(
        description = "Headers set on answers to visitors, Name: value; an empty value removes the header; added to web.response_headers"
    )]
    #[admin(
        ty = map(key = text, value = text),
        label = "Answer headers",
        help = "Headers set on answers, for example Access-Control-Allow-Origin: https://example.com for CORS; an empty value removes one",
        empty_label = "As the web proxy says",
        advanced,
        visible_if(ne(tls, "passthrough"))
    )]
    pub response_headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "A folder with any of 404.html, 413.html, 429.html, 502.html, 503.html, 504.html and maintenance.html, relative to the config file; from web.error_pages when not set"
    )]
    #[admin(
        ty = path(must_exist, relative_to = "config"),
        help = "A folder with this site's own pages, any of 404.html, 413.html, 429.html, 502.html, 503.html, 504.html and maintenance.html; the rest stay built in",
        none_label = "As the web proxy says",
        advanced,
        locked,
        visible_if(ne(tls, "passthrough"))
    )]
    pub error_pages: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
#[admin(semantic = union(enum_(TlsMode), object(TlsFiles)))]
pub enum SiteTls {
    #[admin(label = "Choice")]
    Mode(TlsMode),
    #[admin(label = "My own certificate file")]
    Files(TlsFiles),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum TlsMode {
    #[admin(
        label = "Automatic",
        help = "burrow does HTTPS with a certificate for every name"
    )]
    Auto,
    #[admin(
        label = "The app does its own HTTPS",
        help = "The app keeps its own certificate; burrow only reads the name"
    )]
    Passthrough,
    #[admin(
        label = "Off: plain HTTP",
        help = "No HTTPS: the website is served as plain HTTP"
    )]
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum ChallengeKind {
    #[admin(
        label = "TLS",
        help = "Let's Encrypt connects to the HTTPS port; needs web.https on 443"
    )]
    Tls,
    #[admin(
        label = "HTTP",
        help = "Let's Encrypt fetches a file over plain HTTP; needs web.http on 80"
    )]
    Http,
    #[admin(
        label = "DNS",
        help = "Let's Encrypt reads a DNS record; DNS checks come in a later phase"
    )]
    Dns,
}
