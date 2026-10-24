use serde::{Deserialize, Serialize};

use crate::admin_schema::AdminSchema;

use super::Bind;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[admin(label = "Web proxy")]
pub struct WebConfig {
    #[schemars(
        description = "Port or ip:port for HTTPS, like 443; a port alone means every IPv4 and IPv6 address, 0.0.0.0:443 only IPv4"
    )]
    #[admin(
        ty = ip_port(default_ip = "::"),
        label = "HTTPS port",
        help = "The port where websites and TLS services are reached by name, for example 443; a port alone listens on IPv4 and IPv6, 0.0.0.0:443 on IPv4 only",
        locked
    )]
    pub https: Bind,
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

    pub fn tls(&self) -> Option<WebTls> {
        match self {
            ServiceWeb::Site(s) => s.tls,
            _ => None,
        }
    }
}

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
        description = "passthrough: the app keeps its own TLS and certificate, burrow only reads the name"
    )]
    #[admin(
        label = "TLS",
        help = "Passthrough leaves TLS to the app: it keeps its own certificate, and burrow only reads the name",
        none_label = "burrow handles HTTPS"
    )]
    pub tls: Option<WebTls>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum WebTls {
    #[admin(
        label = "Passthrough",
        help = "The app keeps its own TLS; burrow only reads the name"
    )]
    Passthrough,
}
