use serde::{Deserialize, Serialize};

use crate::admin_schema::AdminSchema;

use super::TlsFiles;

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
