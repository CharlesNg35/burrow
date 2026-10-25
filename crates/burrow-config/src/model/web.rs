use serde::{Deserialize, Serialize};

use crate::admin_schema::AdminSchema;

use super::{Bind, CertFile, LetsEncrypt, SiteTls, WebSite};

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
        none_label = "No HTTPS port",
        locked
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
        none_label = "No HTTP port",
        locked
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
    Site(WebSite),
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
