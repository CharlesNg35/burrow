use serde::{Deserialize, Serialize};

use crate::admin_schema::AdminSchema;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
#[admin(semantic = union(text, object(RedirectRule)))]
pub enum RedirectTarget {
    #[admin(label = "Temporary redirect")]
    To(String),
    #[admin(label = "With options")]
    Rule(RedirectRule),
}

impl RedirectTarget {
    pub fn to(&self) -> &str {
        match self {
            RedirectTarget::To(t) => t,
            RedirectTarget::Rule(r) => &r.to,
        }
    }

    pub fn permanent(&self) -> bool {
        match self {
            RedirectTarget::To(_) => false,
            RedirectTarget::Rule(r) => r.permanent,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[admin(label = "Redirect")]
pub struct RedirectRule {
    #[schemars(
        description = "Where the visitor is sent: a full URL like https://example.com/new, or a path on the same name like /new"
    )]
    #[admin(
        label = "Send to",
        help = "A full address or a path on the same name, for example https://example.com/new or /new; the rest of the path and the query are kept"
    )]
    pub to: String,
    #[serde(default, skip_serializing_if = "super::is_false")]
    #[schemars(
        description = "true: a permanent redirect (308) that browsers remember; false (the default): a temporary one (307)"
    )]
    #[admin(help = "Browsers remember a permanent redirect, so use it only when you are sure")]
    pub permanent: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
#[admin(semantic = union(bool, object(TargetCa), object(TargetUnchecked)))]
pub enum TargetTls {
    #[admin(label = "Checked by public CAs")]
    Checked(bool),
    #[admin(label = "Checked by my CA (file)")]
    Ca(TargetCa),
    #[admin(label = "Not checked")]
    Unchecked(TargetUnchecked),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[admin(label = "The app's CA")]
pub struct TargetCa {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "PEM file of the CA that signed the app's certificate, relative to the config file, for example certs/corp-ca.pem"
    )]
    #[admin(
        ty = path(must_exist, relative_to = "config"),
        label = "CA file",
        help = "The CA that signed the app's certificate, for example certs/corp-ca.pem",
        none_label = "Not set",
        locked
    )]
    pub ca: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "The CA that signed the app's certificate, uploaded in the admin UI, for example ca-1a2b3c4d5e6f7a8b"
    )]
    #[admin(
        ty = upload(ca),
        label = "Uploaded CA",
        help = "The CA that signed the app's certificate, uploaded here",
        none_label = "None"
    )]
    pub ca_upload: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[admin(label = "Unchecked HTTPS")]
pub struct TargetUnchecked {
    #[schemars(
        description = "true: the app's certificate is not checked, for apps with a self-made certificate like Proxmox, UniFi or DSM"
    )]
    #[admin(
        label = "Skip the certificate check",
        help = "For apps with a self-made certificate, like Proxmox, UniFi or DSM"
    )]
    pub insecure_skip_verify: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
#[admin(semantic = union(path(relative_to = "config"), object(AccessLogOptions)))]
pub enum AccessLog {
    #[admin(label = "A file")]
    File(String),
    #[admin(label = "A file and its format")]
    Options(AccessLogOptions),
}

impl AccessLog {
    pub fn file(&self) -> &str {
        match self {
            AccessLog::File(f) => f,
            AccessLog::Options(o) => &o.file,
        }
    }

    pub fn format(&self) -> AccessLogFormat {
        match self {
            AccessLog::File(_) => AccessLogFormat::Common,
            AccessLog::Options(o) => o.format.unwrap_or(AccessLogFormat::Common),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[admin(label = "Access log")]
pub struct AccessLogOptions {
    #[schemars(
        description = "The log file, relative to the config file, for example logs/access.log"
    )]
    #[admin(
        ty = path(relative_to = "config"),
        help = "Where each request is written, one line each, for example logs/access.log"
    )]
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "common (the default, like nginx and Apache) or json")]
    #[admin(
        help = "common is one line like nginx and Apache write; json suits log tools",
        none_label = "common"
    )]
    pub format: Option<AccessLogFormat>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum AccessLogFormat {
    #[admin(label = "Common", help = "One line per request, like nginx and Apache")]
    Common,
    #[admin(label = "JSON", help = "One JSON object per line, for log tools")]
    Json,
}
