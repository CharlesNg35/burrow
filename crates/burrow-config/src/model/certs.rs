use serde::{Deserialize, Serialize};

use crate::admin_schema::AdminSchema;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[admin(label = "Let's Encrypt settings")]
pub struct LetsEncrypt {
    #[schemars(
        description = "The account's contact address, for example ops@example.com; writing it accepts Let's Encrypt's terms"
    )]
    #[admin(
        help = "Let's Encrypt's contact for this server, for example ops@example.com; giving it accepts Let's Encrypt's terms"
    )]
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "production (the default), staging (Let's Encrypt's test service), or the directory URL of another ACME server, like https://ca.internal/acme/directory"
    )]
    #[admin(
        label = "Certificate service",
        help = "Where certificates come from: Let's Encrypt, its test service for trying things out, or another ACME server by its directory URL, for example https://ca.internal/acme/directory",
        none_label = "Let's Encrypt",
        advanced
    )]
    pub directory: Option<Directory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "PEM file of the CA to trust for that ACME server, relative to the config file, for example certs/internal-ca.pem"
    )]
    #[admin(
        ty = path(must_exist, relative_to = "config"),
        label = "Trusted CA file",
        help = "The CA certificate to trust for another ACME server, for example certs/internal-ca.pem",
        none_label = "The public CAs",
        advanced,
        locked
    )]
    pub ca: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
#[admin(semantic = union(enum_(DirectoryName), text))]
pub enum Directory {
    #[admin(label = "Let's Encrypt")]
    Named(DirectoryName),
    #[admin(label = "Another ACME server")]
    Url(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum DirectoryName {
    #[admin(label = "Production", help = "Real certificates that browsers trust")]
    Production,
    #[admin(
        label = "Staging",
        help = "Let's Encrypt's test service: higher limits, certificates browsers do not trust"
    )]
    Staging,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[admin(label = "Certificate file")]
pub struct CertFile {
    #[schemars(
        description = "The names this file is used for, each listed in one entry only, for example [\"*.example.com\", example.com]"
    )]
    #[admin(
        ty = list(host_name),
        label = "Host names",
        help = "The names this file is used for, for example *.example.com and example.com; the certificate must cover each"
    )]
    pub hosts: Vec<String>,
    #[schemars(
        description = "Certificate chain PEM file, relative to the config file, for example certs/fullchain.pem"
    )]
    #[admin(
        ty = path(must_exist, relative_to = "config"),
        label = "Certificate file",
        locked
    )]
    pub cert: String,
    #[schemars(
        description = "Private key PEM file, relative to the config file, for example certs/privkey.pem"
    )]
    #[admin(
        ty = path(must_exist, relative_to = "config"),
        label = "Private key file",
        locked
    )]
    pub key: String,
}
