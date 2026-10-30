use burrow_admin_proto::serde_json::{Map, Value};
use burrow_config::{Entry, ForwardAuthConfig, ServerConfig, ServiceEntry, WebConfig, WebSite};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    pub kind: &'static str,
    pub name: &'static str,
    pub path: &'static str,
    pub headers: &'static [&'static str],
    pub note: &'static str,
}

pub const OTHER: &str = "Other";

pub const PRESETS: [Preset; 3] = [
    Preset {
        kind: "Authelia",
        name: "authelia",
        path: "/api/authz/forward-auth",
        headers: &[
            "Remote-User",
            "Remote-Groups",
            "Remote-Email",
            "Remote-Name",
        ],
        note: "",
    },
    Preset {
        kind: "authentik",
        name: "authentik",
        path: "/outpost.goauthentik.io/auth/caddy",
        headers: &[
            "X-Authentik-Username",
            "X-Authentik-Groups",
            "X-Authentik-Email",
            "X-Authentik-Name",
            "X-Authentik-Uid",
        ],
        note: "Needs a Proxy provider in forward auth mode on authentik's embedded outpost.",
    },
    Preset {
        kind: "oauth2-proxy",
        name: "oauth2-proxy",
        path: "/oauth2/auth",
        headers: &["X-Auth-Request-User", "X-Auth-Request-Email"],
        note: "Start oauth2-proxy with --reverse-proxy and --set-xauthrequest so it sends these headers.",
    },
];

pub fn kinds() -> Vec<&'static str> {
    PRESETS.iter().map(|p| p.kind).chain([OTHER]).collect()
}

pub fn preset(kind: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.kind == kind)
}

pub fn kind_of(path: &str) -> &'static str {
    PRESETS
        .iter()
        .find(|p| p.path == path.trim())
        .map_or(OTHER, |p| p.kind)
}

pub fn logins(doc: &Value) -> Map<String, Value> {
    ServerConfig::WEB
        .get(doc)
        .and_then(|w| WebConfig::FORWARD_AUTH.get(w))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

pub fn chosen(site: &Value, logins: &Map<String, Value>) -> Option<String> {
    let setting: burrow_config::ForwardAuthSetting =
        burrow_admin_proto::serde_json::from_value(WebSite::FORWARD_AUTH.get(site)?.clone())
            .ok()?;
    setting
        .chosen_name(logins.keys().map(String::as_str))
        .map(str::to_string)
}

pub fn used_by(doc: &Value, name: &str) -> Vec<String> {
    let all = logins(doc);
    ServiceEntry::entries(doc)
        .map(|m| {
            m.iter()
                .filter(|(_, svc)| {
                    ServiceEntry::WEB
                        .get(svc)
                        .and_then(|w| chosen(w, &all))
                        .is_some_and(|n| n == name)
                })
                .map(|(k, _)| k.clone())
                .collect()
        })
        .unwrap_or_default()
}

pub fn config(v: &Value) -> Option<ForwardAuthConfig> {
    burrow_admin_proto::serde_json::from_value(v.clone()).ok()
}

pub fn summary(c: &ForwardAuthConfig) -> String {
    match kind_of(&c.path) {
        OTHER => format!("Service {}, path {}", c.service, c.path),
        kind => format!("{kind} on service {}, path {}", c.service, c.path),
    }
}

pub fn passes(c: &ForwardAuthConfig) -> String {
    if c.copy_headers.is_empty() {
        return "no login headers".to_string();
    }
    let shown: Vec<&str> = c.copy_headers.iter().take(3).map(String::as_str).collect();
    match c.copy_headers.len().saturating_sub(3) {
        0 => shown.join(", "),
        more => format!("{} (+{more})", shown.join(", ")),
    }
}

pub fn note(name: &str, c: &ForwardAuthConfig) -> String {
    let gets = if c.copy_headers.is_empty() {
        "no login headers".to_string()
    } else {
        c.copy_headers.join(", ")
    };
    format!("{name}: {}; the app gets {gets}", summary(c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::serde_json::json;

    fn doc() -> Value {
        json!({
            "web": {"forward_auth": {
                "sso": {"service": "authelia", "path": "/api/authz/forward-auth", "copy_headers": ["Remote-User"]},
                "ak": {"service": "authentik", "path": "/outpost.goauthentik.io/auth/caddy"}
            }},
            "services": {
                "wiki": {"web": {"hosts": ["w.example.com"], "forward_auth": "sso"}},
                "dav": {"web": {"hosts": ["d.example.com"], "forward_auth": " sso "}},
                "blog": {"web": {"hosts": ["b.example.com"], "forward_auth": "ak"}},
                "shop": {"web": {"hosts": ["s.example.com"], "forward_auth": true}},
                "api": {"target": 1}
            }
        })
    }

    #[test]
    fn every_preset_is_a_valid_login_service_and_other_is_last() {
        assert_eq!(kinds(), ["Authelia", "authentik", "oauth2-proxy", OTHER]);
        for p in PRESETS {
            assert!(
                burrow_syntax::name::check_name(p.name).is_ok(),
                "{}",
                p.name
            );
            assert!(
                burrow_syntax::url_path::route_path(p.path).is_ok(),
                "{}",
                p.path
            );
            for h in p.headers {
                assert!(burrow_syntax::web::headers::is_name(h), "{h}");
            }
            assert_eq!(kind_of(p.path), p.kind);
        }
        assert_eq!(kind_of("/check"), OTHER);
        assert!(preset(OTHER).is_none());
    }

    #[test]
    fn the_path_help_gives_the_same_examples_as_the_presets() {
        use burrow_config::admin_schema::AdminSchema;
        let node = ForwardAuthConfig::node();
        let help = &burrow_admin_proto::schema::Node::at(&node, &["path".to_string()])
            .expect("path")
            .help;
        for p in &PRESETS[..2] {
            assert!(help.contains(p.path), "{} in {help}", p.path);
        }
    }

    #[test]
    fn a_login_service_is_used_by_the_sites_that_name_it() {
        let d = doc();
        assert_eq!(
            used_by(&d, "sso"),
            ["wiki", "dav"],
            "in the order of the file"
        );
        assert_eq!(used_by(&d, "ak"), ["blog"]);
        let one = json!({
            "web": {"forward_auth": {"sso": {"service": "a", "path": "/x"}}},
            "services": {"shop": {"web": {"hosts": ["s.example.com"], "forward_auth": true}}}
        });
        assert_eq!(used_by(&one, "sso"), ["shop"], "true means the only one");
    }

    #[test]
    fn the_card_and_the_site_note_say_what_the_login_does() {
        let c = config(&doc()["web"]["forward_auth"]["sso"]).unwrap();
        assert_eq!(
            summary(&c),
            "Authelia on service authelia, path /api/authz/forward-auth"
        );
        assert_eq!(passes(&c), "Remote-User");
        assert_eq!(
            note("sso", &c),
            "sso: Authelia on service authelia, path /api/authz/forward-auth; the app gets Remote-User"
        );
        let many = ForwardAuthConfig {
            copy_headers: PRESETS[1].headers.iter().map(|h| h.to_string()).collect(),
            ..c
        };
        assert_eq!(
            passes(&many),
            "X-Authentik-Username, X-Authentik-Groups, X-Authentik-Email (+2)"
        );
    }
}
