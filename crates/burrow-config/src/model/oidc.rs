use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::admin_schema::AdminSchema;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[admin(label = "Login provider")]
pub struct OidcConfig {
    #[schemars(
        description = "The provider's issuer URL, where /.well-known/openid-configuration is found, for example https://accounts.google.com"
    )]
    #[admin(
        help = "The provider's address, for example https://accounts.google.com or https://sso.example.com/realms/home"
    )]
    pub issuer: String,
    #[schemars(description = "The client ID the provider gave this server")]
    #[admin(
        label = "Client ID",
        help = "The client ID you got when you registered burrow with the provider"
    )]
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "The client secret, from ${GOOGLE_SECRET} or file:path; leave it out for a public client"
    )]
    #[admin(
        ty = secret_ref(max = 1, reference),
        help = "The client secret from the provider, from an environment variable like ${GOOGLE_SECRET} or a file like file:/etc/burrow/google.secret",
        none_label = "None (public client)",
        locked
    )]
    pub client_secret: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(
    untagged,
    expecting = "name a login provider from web.oidc, like oidc: google, or a list of them"
)]
#[admin(semantic = union(bool, ref_(oidc), ref_list(oidc)))]
pub enum OidcSetting {
    #[admin(label = "The only login provider")]
    On(bool),
    #[admin(label = "A login provider")]
    Name(String),
    #[admin(label = "Several login providers")]
    Names(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum OidcPick<'a> {
    Off,
    Found(Vec<&'a str>),
    Unknown(&'a str),
    NoneDefined,
    Unclear,
    Empty,
}

impl OidcSetting {
    pub fn on(&self) -> bool {
        !matches!(self, OidcSetting::On(false))
    }

    pub fn names(&self) -> Vec<&str> {
        match self {
            OidcSetting::On(_) => Vec::new(),
            OidcSetting::Name(n) => vec![n.trim()],
            OidcSetting::Names(list) => list.iter().map(|n| n.trim()).collect(),
        }
    }

    pub fn pick<'a, T>(&'a self, all: &'a BTreeMap<String, T>) -> OidcPick<'a> {
        match self {
            OidcSetting::On(false) => OidcPick::Off,
            OidcSetting::On(true) => match all.len() {
                0 => OidcPick::NoneDefined,
                1 => OidcPick::Found(all.keys().map(String::as_str).collect()),
                _ => OidcPick::Unclear,
            },
            OidcSetting::Names(list) if list.is_empty() => OidcPick::Empty,
            _ => {
                let mut found = Vec::new();
                for n in self.names() {
                    match all.get_key_value(n) {
                        Some((k, _)) if !found.contains(&k.as_str()) => found.push(k.as_str()),
                        Some(_) => {}
                        None => return OidcPick::Unknown(n),
                    }
                }
                OidcPick::Found(found)
            }
        }
    }

    pub fn renamed(&mut self, from: &str, to: &str) -> bool {
        match self {
            OidcSetting::Name(n) if n.trim() == from => *n = to.to_string(),
            OidcSetting::Names(list) if list.iter().any(|n| n.trim() == from) => {
                for n in list.iter_mut().filter(|n| n.trim() == from) {
                    *n = to.to_string();
                }
            }
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::serde_json;

    fn providers(names: &[&str]) -> BTreeMap<String, ()> {
        names.iter().map(|n| ((*n).to_string(), ())).collect()
    }

    #[test]
    fn a_site_picks_providers_by_name_by_list_or_by_true_when_there_is_one() {
        let none = providers(&[]);
        let one = providers(&["google"]);
        let two = providers(&["google", "work"]);
        let on = OidcSetting::On(true);
        assert_eq!(on.pick(&none), OidcPick::NoneDefined);
        assert_eq!(on.pick(&one), OidcPick::Found(vec!["google"]));
        assert_eq!(on.pick(&two), OidcPick::Unclear);
        assert_eq!(OidcSetting::On(false).pick(&two), OidcPick::Off);
        let named = OidcSetting::Name(" work ".into());
        assert_eq!(named.pick(&two), OidcPick::Found(vec!["work"]));
        assert_eq!(named.pick(&one), OidcPick::Unknown("work"));
        let list = OidcSetting::Names(vec!["work".into(), "google".into(), "work".into()]);
        assert_eq!(list.pick(&two), OidcPick::Found(vec!["work", "google"]));
        assert_eq!(OidcSetting::Names(Vec::new()).pick(&two), OidcPick::Empty);
    }

    #[test]
    fn a_site_takes_true_a_name_or_a_list() {
        let parse = |s: &str| serde_json::from_str::<OidcSetting>(s);
        assert_eq!(parse("true").unwrap(), OidcSetting::On(true));
        assert_eq!(parse("\"g\"").unwrap(), OidcSetting::Name("g".into()));
        assert_eq!(
            parse("[\"g\", \"w\"]").unwrap(),
            OidcSetting::Names(vec!["g".into(), "w".into()])
        );
        assert_eq!(
            parse("{\"issuer\": \"x\"}").unwrap_err().to_string(),
            "name a login provider from web.oidc, like oidc: google, or a list of them"
        );
    }

    #[test]
    fn a_rename_follows_the_name_in_a_list_or_alone() {
        let mut one = OidcSetting::Name("g".into());
        assert!(one.renamed("g", "google"));
        assert_eq!(one, OidcSetting::Name("google".into()));
        let mut list = OidcSetting::Names(vec!["w".into(), "g".into()]);
        assert!(list.renamed("g", "google"));
        assert_eq!(list.names(), ["w", "google"]);
        assert!(!OidcSetting::On(true).renamed("g", "x"));
    }
}
