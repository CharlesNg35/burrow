use serde::{Deserialize, Serialize};

use crate::admin_schema::AdminSchema;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[admin(label = "Login service")]
pub struct ForwardAuthConfig {
    #[schemars(description = "The service that checks each visitor, for example authelia")]
    #[admin(
        ty = ref_(service),
        help = "The service of the login portal that checks each visitor, for example authelia"
    )]
    pub service: String,
    #[schemars(
        description = "The path on that service that answers the check, for example /api/authz/forward-auth"
    )]
    #[admin(
        ty = url_path,
        help = "Where the login service answers the check, for example /api/authz/forward-auth for Authelia or /outpost.goauthentik.io/auth/nginx for authentik"
    )]
    pub path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(
        description = "Headers of the login service's answer passed on to the app, for example [Remote-User, Remote-Groups]"
    )]
    #[admin(
        ty = list(text),
        label = "Headers to the app",
        help = "Headers from the login service's answer that the app receives, for example Remote-User; a visitor's own copies are always removed",
        empty_label = "None"
    )]
    pub copy_headers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
#[admin(semantic = union(bool, object(ForwardAuthConfig)))]
pub enum ForwardAuthSetting {
    #[admin(label = "The web proxy's login service")]
    Shared(bool),
    #[admin(label = "Its own login service")]
    Own(ForwardAuthConfig),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[admin(label = "Device certificates")]
pub struct ClientAuthFiles {
    #[schemars(
        description = "PEM file with the CA (and its intermediates) that signs the devices' certificates, relative to the config file, for example certs/devices-ca.pem"
    )]
    #[admin(
        ty = path(must_exist, relative_to = "config"),
        label = "CA file",
        help = "The CA that signs your devices' certificates, with its intermediates, for example certs/devices-ca.pem",
        locked
    )]
    pub ca: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Revocation lists (PEM or DER) of that CA, re-read when the file changes, for example certs/devices.crl"
    )]
    #[admin(
        ty = path(must_exist, relative_to = "config"),
        label = "Revocation list",
        help = "Devices you revoked, as the CA publishes them, for example certs/devices.crl; re-read when the file changes",
        none_label = "None",
        locked
    )]
    pub crl: Option<String>,
}
