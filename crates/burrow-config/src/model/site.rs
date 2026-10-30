use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::admin_schema::AdminSchema;

use super::{ClientAuthFiles, ForwardAuthSetting, RedirectTarget, TargetTls};

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
        help = "Who does HTTPS for this website: burrow with an automatic certificate, the app itself (burrow only reads the name), nobody (plain HTTP), burrow with its local CA for names on your network, or burrow with your own certificate",
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
        enabled_if(
            all(
                ne(tls, "passthrough"),
                any(ne(tls, "off"), set("/web/trusted_proxies"))
            ),
            "This needs burrow to do the HTTPS, or Trusted proxies for a plain HTTP site"
        )
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
        enabled_if(
            all(ne(tls, "passthrough"), ne(tls, "off")),
            "This needs burrow to do the HTTPS; this site uses passthrough or plain HTTP"
        )
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
        enabled_if(ne(tls, "passthrough"), "This needs burrow to do the HTTPS; this site uses passthrough")
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
        enabled_if(all(ne(tls, "passthrough"), len_ge(paths, 1)), "This needs burrow to do the HTTPS and at least one path")
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
        enabled_if(ne(tls, "passthrough"), "This needs burrow to do the HTTPS; this site uses passthrough")
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
        enabled_if(ne(tls, "passthrough"), "This needs burrow to do the HTTPS; this site uses passthrough")
    )]
    pub redirects: BTreeMap<String, RedirectTarget>,
    #[serde(default, skip_serializing_if = "super::is_false")]
    #[schemars(
        description = "true answers every request with the maintenance page (503 with Retry-After)"
    )]
    #[admin(
        help = "Answer every visitor with the maintenance page instead of the app",
        enabled_if(
            ne(tls, "passthrough"),
            "This needs burrow to do the HTTPS; this site uses passthrough"
        )
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
        enabled_if(
            ne(tls, "passthrough"),
            "This needs burrow to do the HTTPS; this site uses passthrough"
        )
    )]
    pub target_tls: Option<TargetTls>,
    #[serde(default, skip_serializing_if = "super::is_false")]
    #[schemars(
        description = "true speaks HTTP/2 to the app: h2c, or h2 inside target_tls; for gRPC apps"
    )]
    #[admin(
        label = "HTTP/2 to the app",
        help = "Speak HTTP/2 to the app, needed for gRPC: without HTTPS to the app it is plain h2c, with it h2 inside TLS",
        advanced,
        enabled_if(
            ne(tls, "passthrough"),
            "This needs burrow to do the HTTPS; this site uses passthrough"
        )
    )]
    pub target_http2: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(
        description = "Users and their password hashes, user: hash, argon2id from burrow passwd or bcrypt from htpasswd -B"
    )]
    #[admin(
        ty = map(key = text, value = password_hash, value_help = "The password hash, for example the line burrow passwd alice prints"),
        label = "Passwords",
        help = "Visitors sign in with a user name and password; the password is stored only as a hash",
        empty_label = "None",
        enabled_if(ne(tls, "passthrough"), "This needs burrow to do the HTTPS; this site uses passthrough")
    )]
    pub basic_auth: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Visitors log in at a login service first: the name of one under web.forward_auth, or true when there is only one"
    )]
    #[admin(
        label = "Login service",
        help = "Visitors log in at a single sign-on service like Authelia or authentik before they reach the app",
        none_label = "Off",
        enabled_if(
            ne(tls, "passthrough"),
            "This needs burrow to do the HTTPS; this site uses passthrough"
        )
    )]
    pub forward_auth: Option<ForwardAuthSetting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Requests per second per visitor address (IPv6 per /64), bursts of twice that, 429 above; from web.rate_limit when not set"
    )]
    #[schemars(range(min = 1))]
    #[admin(
        ty = integer(unit = "requests per second"),
        help = "How many requests per second one visitor may send, for example 20; more get 429",
        none_label = "As the web proxy says",
        advanced,
        enabled_if(ne(tls, "passthrough"), "This needs burrow to do the HTTPS; this site uses passthrough")
    )]
    pub rate_limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Only devices with a certificate signed by this CA get in, checked in the TLS handshake: {ca, crl}"
    )]
    #[admin(
        label = "Device certificates",
        help = "Only devices with a certificate from your own CA may open the site, for example company laptops",
        none_label = "Off",
        advanced,
        enabled_if(
            all(ne(tls, "passthrough"), ne(tls, "off")),
            "This needs burrow to do the HTTPS; this site uses passthrough or plain HTTP"
        )
    )]
    pub client_auth: Option<ClientAuthFiles>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "true sends Strict-Transport-Security for a year on this site's HTTPS answers; from web.hsts when not set"
    )]
    #[admin(
        label = "HSTS",
        help = "Tell browsers to use only HTTPS for this site for a year",
        none_label = "As the web proxy says",
        advanced,
        enabled_if(
            all(ne(tls, "passthrough"), ne(tls, "off")),
            "This needs burrow to do the HTTPS; this site uses passthrough or plain HTTP"
        )
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
        enabled_if(
            ne(tls, "passthrough"),
            "This needs burrow to do the HTTPS; this site uses passthrough"
        )
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
        enabled_if(
            ne(tls, "passthrough"),
            "This needs burrow to do the HTTPS; this site uses passthrough"
        )
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
        enabled_if(
            ne(tls, "passthrough"),
            "This needs burrow to do the HTTPS; this site uses passthrough"
        )
    )]
    pub response_header_timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(
        description = "Headers set on requests to the app, Name: value, also Host; an empty value removes the header; added to web.request_headers"
    )]
    #[admin(
        ty = map(key = text, value = text, value_help = "The header's value, for example DENY; empty removes the header"),
        help = "Headers set on requests to the app, for example Host: app.internal for an app that wants its own name; an empty value removes one",
        empty_label = "As the web proxy says",
        advanced,
        enabled_if(ne(tls, "passthrough"), "This needs burrow to do the HTTPS; this site uses passthrough")
    )]
    pub request_headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(
        description = "Headers set on answers to visitors, Name: value; an empty value removes the header; added to web.response_headers"
    )]
    #[admin(
        ty = map(key = text, value = text, value_help = "The header's value, for example DENY; empty removes the header"),
        label = "Answer headers",
        help = "Headers set on answers, for example Access-Control-Allow-Origin: https://example.com for CORS; an empty value removes one",
        empty_label = "As the web proxy says",
        advanced,
        enabled_if(ne(tls, "passthrough"), "This needs burrow to do the HTTPS; this site uses passthrough")
    )]
    pub response_headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "A folder with any of 404.html, 413.html, 429.html, 502.html, 503.html, 504.html and maintenance.html, relative to the config file; from web.error_pages when not set"
    )]
    #[admin(
        ty = path(must_exist, relative_to = "config"),
        help = "A folder with this site's own pages, for example pages/shop/, holding any of 404.html, 413.html, 429.html, 502.html, 503.html, 504.html and maintenance.html; the rest stay built in",
        none_label = "As the web proxy says",
        advanced,
        locked,
        enabled_if(ne(tls, "passthrough"), "This needs burrow to do the HTTPS; this site uses passthrough")
    )]
    pub error_pages: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
#[admin(semantic = union(enum_(TlsMode), object(SiteFiles)))]
pub enum SiteTls {
    #[admin(label = "Choice")]
    Mode(TlsMode),
    #[admin(label = "My own certificate")]
    Files(SiteFiles),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[admin(label = "My own certificate")]
pub struct SiteFiles {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Certificate chain PEM file, relative to the config file, for example certs/fullchain.pem; with key, or upload instead"
    )]
    #[admin(
        ty = path(must_exist, relative_to = "config"),
        label = "Certificate file",
        none_label = "Not set",
        locked
    )]
    pub cert: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Private key PEM file, relative to the config file, for example certs/privkey.pem"
    )]
    #[admin(
        ty = path(must_exist, relative_to = "config"),
        label = "Private key file",
        none_label = "Not set",
        locked
    )]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "A certificate and its key uploaded in the admin UI, kept in the state folder, for example pair-1a2b3c4d5e6f7a8b"
    )]
    #[admin(
        ty = upload(pair),
        label = "Uploaded certificate",
        help = "A certificate with its private key, uploaded here and kept in burrow's state folder",
        none_label = "None"
    )]
    pub upload: Option<String>,
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
    #[admin(
        label = "Local CA, for names on your network",
        help = "burrow's own certificate authority signs certificates for names like nas.lan; install its certificate on your devices once"
    )]
    Internal,
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
        help = "Let's Encrypt reads a DNS record that burrow sets through web.letsencrypt.dns; no open port needed"
    )]
    Dns,
}
