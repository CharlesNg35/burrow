use std::collections::BTreeMap;

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
        help = "Where the login service answers the check, for example /api/authz/forward-auth for Authelia or /outpost.goauthentik.io/auth/caddy for authentik"
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
#[serde(
    untagged,
    expecting = "name a login service from web.forward_auth, like forward_auth: authelia"
)]
#[admin(semantic = union(bool, ref_(login)))]
pub enum ForwardAuthSetting {
    #[admin(label = "The only login service")]
    On(bool),
    #[admin(label = "A login service")]
    Name(String),
}

pub const RESERVED_LOGIN_NAMES: [&str; 2] = ["true", "false"];

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LoginPick<'a> {
    Off,
    Found(&'a str, &'a ForwardAuthConfig),
    Unknown(&'a str),
    NoneDefined,
    Unclear,
}

impl ForwardAuthSetting {
    pub fn on(&self) -> bool {
        !matches!(self, ForwardAuthSetting::On(false))
    }

    pub fn chosen_name<'a>(
        &'a self,
        mut names: impl ExactSizeIterator<Item = &'a str>,
    ) -> Option<&'a str> {
        match self {
            ForwardAuthSetting::On(false) => None,
            ForwardAuthSetting::On(true) => (names.len() == 1).then(|| names.next()).flatten(),
            ForwardAuthSetting::Name(n) => Some(n.trim()),
        }
    }

    pub fn pick<'a>(&'a self, logins: &'a BTreeMap<String, ForwardAuthConfig>) -> LoginPick<'a> {
        match (self, logins.len()) {
            (ForwardAuthSetting::On(false), _) => LoginPick::Off,
            (ForwardAuthSetting::On(true), 0) => LoginPick::NoneDefined,
            (ForwardAuthSetting::On(true), n) if n > 1 => LoginPick::Unclear,
            _ => match self.chosen_name(logins.keys().map(String::as_str)) {
                Some(n) => logins
                    .get_key_value(n)
                    .map_or(LoginPick::Unknown(n), |(k, f)| LoginPick::Found(k, f)),
                None => LoginPick::NoneDefined,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::serde_json;

    fn login(service: &str) -> ForwardAuthConfig {
        ForwardAuthConfig {
            service: service.into(),
            path: "/auth".into(),
            copy_headers: Vec::new(),
        }
    }

    #[test]
    fn a_site_picks_its_login_service_by_name_or_by_true_when_there_is_one() {
        let mut logins = BTreeMap::new();
        let named = ForwardAuthSetting::Name(" authelia ".into());
        let on = ForwardAuthSetting::On(true);
        assert_eq!(on.pick(&logins), LoginPick::NoneDefined);
        assert_eq!(named.pick(&logins), LoginPick::Unknown("authelia"));
        logins.insert("authelia".to_string(), login("authelia"));
        let one = LoginPick::Found("authelia", &logins["authelia"]);
        assert_eq!(on.pick(&logins), one);
        assert_eq!(named.pick(&logins), one);
        assert_eq!(ForwardAuthSetting::On(false).pick(&logins), LoginPick::Off);
        logins.insert("authentik".to_string(), login("authentik"));
        assert_eq!(on.pick(&logins), LoginPick::Unclear);
        assert!(matches!(
            named.pick(&logins),
            LoginPick::Found("authelia", _)
        ));
    }

    #[test]
    fn a_site_takes_true_false_or_a_name_and_explains_the_old_inline_block() {
        let parse = |s: &str| serde_json::from_str::<ForwardAuthSetting>(s);
        assert_eq!(parse("true").unwrap(), ForwardAuthSetting::On(true));
        assert_eq!(
            parse("\"sso\"").unwrap(),
            ForwardAuthSetting::Name("sso".into())
        );
        let e = parse(r#"{"service": "authelia", "path": "/auth"}"#).unwrap_err();
        assert_eq!(
            e.to_string(),
            "name a login service from web.forward_auth, like forward_auth: authelia"
        );
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[admin(label = "Device certificates")]
pub struct ClientAuthFiles {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "PEM file with the CA (and its intermediates) that signs the devices' certificates, relative to the config file, for example certs/devices-ca.pem"
    )]
    #[admin(
        ty = path(must_exist, relative_to = "config"),
        label = "CA file",
        help = "The CA that signs your devices' certificates, with its intermediates, for example certs/devices-ca.pem",
        none_label = "Not set",
        locked
    )]
    pub ca: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "The devices' CA, uploaded in the admin UI, for example ca-1a2b3c4d5e6f7a8b"
    )]
    #[admin(
        ty = upload(ca),
        label = "Uploaded CA",
        help = "The CA that signs your devices' certificates, with its intermediates, uploaded here",
        none_label = "None"
    )]
    pub ca_upload: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "The CA's revocation lists, uploaded in the admin UI, for example crl-1a2b3c4d5e6f7a8b"
    )]
    #[admin(
        ty = upload(crl),
        label = "Uploaded revocation list",
        help = "Devices you revoked, as the CA publishes them, uploaded here; upload a new list to replace it",
        none_label = "None"
    )]
    pub crl_upload: Option<String>,
}
