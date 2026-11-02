use std::net::SocketAddr;

use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::status::Status;
use burrow_config::{ClientEntry, Entry, SelfEntry, ServerConfig, ServiceEntry, VisitorEntry};

use crate::wording::{OFFLINE, ONLINE, THIS_SERVER};

pub const SELF_REACH_OFF: &str = "first set May reach on this server, in Settings, Server";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    Here,
    Online,
    Offline,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientChoice {
    pub name: String,
    pub label: String,
    pub detail: String,
    pub presence: Presence,
    pub blocked: Option<&'static str>,
}

pub fn self_reach_ready(doc: &Value) -> bool {
    ServerConfig::SELF
        .then(SelfEntry::ALLOW)
        .get(doc)
        .and_then(Value::as_array)
        .is_some_and(|a| !a.is_empty())
}

pub fn label_of(name: &str, reserved: &[String]) -> String {
    if reserved.iter().any(|r| r == name) {
        THIS_SERVER.to_string()
    } else {
        name.to_string()
    }
}

pub fn peer_ip(from: &str) -> String {
    from.parse::<SocketAddr>()
        .map_or_else(|_| from.to_string(), |a| a.ip().to_string())
}

pub fn default_client(doc: &Value, reserved: &[String]) -> Option<String> {
    ClientEntry::names(doc)
        .into_iter()
        .next()
        .or_else(|| reserved.first().cloned())
}

pub fn client_choices(
    doc: &Value,
    status: Option<&Status>,
    reserved: &[String],
    field: &[String],
) -> Vec<ClientChoice> {
    let blocked =
        (VisitorEntry::REACH.is(field) && !self_reach_ready(doc)).then_some(SELF_REACH_OFF);
    let local = reserved.iter().map(|n| ClientChoice {
        name: n.clone(),
        label: THIS_SERVER.to_string(),
        detail: blocked.map_or_else(|| n.clone(), |why| format!("{n} · {why}")),
        presence: Presence::Here,
        blocked,
    });
    let remote = ClientEntry::names(doc).into_iter().map(|n| {
        let seen = status.map(|s| s.clients.iter().find(|c| c.name == n));
        let (presence, detail) = match seen {
            None => (Presence::Unknown, String::new()),
            Some(Some(c)) if c.connected => (
                Presence::Online,
                c.from.as_deref().map_or_else(
                    || ONLINE.to_string(),
                    |f| format!("{ONLINE} · {}", peer_ip(f)),
                ),
            ),
            Some(_) => (Presence::Offline, OFFLINE.to_string()),
        };
        ClientChoice {
            label: n.clone(),
            name: n,
            detail,
            presence,
            blocked: None,
        }
    });
    local.chain(remote).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceChoice {
    pub name: String,
    pub detail: String,
    pub private: bool,
    pub up: Option<bool>,
}

pub fn service_choices(doc: &Value, status: Option<&Status>) -> Vec<ServiceChoice> {
    let online = |c: &String| {
        status
            .and_then(|s| s.clients.iter().find(|x| &x.name == c))
            .is_some_and(|x| x.connected)
    };
    ServiceEntry::entries(doc)
        .map(|m| {
            m.iter()
                .map(|(n, v)| ServiceChoice {
                    name: n.clone(),
                    detail: crate::web::reached(v),
                    private: ServiceEntry::BIND.get(v).is_none_or(Value::is_null),
                    up: status.map(|_| ServiceEntry::client_names(v).iter().any(online)),
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::serde_json::json;
    use burrow_admin_proto::status::ClientStatus;

    fn path(p: &[&str]) -> Vec<String> {
        p.iter().map(|s| s.to_string()).collect()
    }

    fn reserved() -> Vec<String> {
        vec!["self".into()]
    }

    fn status() -> Status {
        Status {
            clients: vec![
                ClientStatus {
                    name: "home".into(),
                    connected: true,
                    connected_secs: Some(5400),
                    from: Some("82.66.220.1:51234".into()),
                    ..ClientStatus::default()
                },
                ClientStatus {
                    name: "office".into(),
                    left_secs_ago: Some(60),
                    ..ClientStatus::default()
                },
                ClientStatus {
                    name: "v6".into(),
                    connected: true,
                    from: Some("[2001:db8:85a3::8a2e:370:7334]:443".into()),
                    ..ClientStatus::default()
                },
            ],
            ..Status::default()
        }
    }

    fn doc() -> Value {
        json!({"clients": {"home": {}, "office": {}, "v6": {}}})
    }

    #[test]
    fn a_service_client_offers_this_server_first_then_every_client() {
        let c = client_choices(
            &doc(),
            None,
            &reserved(),
            &path(&["services", "*", "client"]),
        );
        let names: Vec<&str> = c.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["self", "home", "office", "v6"]);
        assert_eq!(c[0].label, "This server");
        assert_eq!(c[0].detail, "self");
        assert_eq!(c[0].presence, Presence::Here);
        assert!(
            c.iter().all(|c| c.blocked.is_none()),
            "self needs no self.allow to run a service"
        );
        assert!(
            c[1..]
                .iter()
                .all(|c| c.presence == Presence::Unknown && c.detail.is_empty())
        );
    }

    #[test]
    fn every_service_field_that_names_a_client_offers_this_server() {
        for field in [
            path(&["services", "*", "client"]),
            path(&["services", "*", "clients"]),
            path(&["services", "*", "clients", "*", "client"]),
        ] {
            let c = client_choices(&json!({}), None, &reserved(), &field);
            assert_eq!(c.len(), 1, "{field:?}");
            assert_eq!(
                (c[0].name.as_str(), c[0].blocked),
                ("self", None),
                "{field:?}"
            );
        }
    }

    #[test]
    fn a_client_says_online_with_its_address_or_just_offline() {
        let st = status();
        let c = client_choices(
            &doc(),
            Some(&st),
            &reserved(),
            &path(&["services", "*", "client"]),
        );
        assert_eq!(c[1].detail, "online · 82.66.220.1");
        assert_eq!(c[1].presence, Presence::Online);
        assert_eq!(c[2].detail, "offline");
        assert_eq!(c[2].presence, Presence::Offline);
        assert_eq!(c[3].detail, "online · 2001:db8:85a3::8a2e:370:7334");
    }

    #[test]
    fn reach_offers_this_server_disabled_until_it_may_reach_something() {
        let field = path(&["visitors", "*", "reach"]);
        let off = client_choices(&doc(), None, &reserved(), &field);
        assert_eq!(off[0].blocked, Some(SELF_REACH_OFF));
        assert!(off[0].detail.contains(SELF_REACH_OFF));
        let empty = json!({"self": {"allow": []}, "clients": {}});
        assert!(
            client_choices(&empty, None, &reserved(), &field)[0]
                .blocked
                .is_some()
        );
        let on = json!({"self": {"allow": ["127.0.0.1/32"]}, "clients": {}});
        let c = client_choices(&on, None, &reserved(), &field);
        assert_eq!((c[0].blocked, c[0].detail.as_str()), (None, "self"));
    }

    #[test]
    fn an_older_server_without_self_offers_only_its_clients() {
        let c = client_choices(&doc(), None, &[], &path(&["services", "*", "client"]));
        assert!(c.iter().all(|c| c.name != "self"));
    }

    #[test]
    fn a_new_service_starts_on_the_first_client_or_on_this_server_without_clients() {
        assert_eq!(default_client(&doc(), &reserved()).as_deref(), Some("home"));
        assert_eq!(
            default_client(&json!({}), &reserved()).as_deref(),
            Some("self")
        );
        assert_eq!(default_client(&json!({}), &[]), None);
    }

    #[test]
    fn services_say_how_they_are_reached_and_whether_a_client_is_up() {
        let doc = json!({"services": {
            "db": {"client": "office", "target": 5432, "visitors": ["laptop"]},
            "web": {"clients": ["home", "office"], "bind": 8080, "target": 3000}
        }});
        let st = status();
        let s = service_choices(&doc, Some(&st));
        assert_eq!(
            s.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["db", "web"]
        );
        assert_eq!((s[0].private, s[0].up), (true, Some(false)));
        assert_eq!((s[1].private, s[1].up), (false, Some(true)));
        assert!(!s[1].detail.is_empty());
        assert!(service_choices(&doc, None).iter().all(|c| c.up.is_none()));
    }

    #[test]
    fn labels_and_addresses_read_plainly() {
        assert_eq!(label_of("self", &reserved()), "This server");
        assert_eq!(label_of("home", &reserved()), "home");
        assert_eq!(label_of("self", &[]), "self");
        assert_eq!(peer_ip("10.0.0.1:9"), "10.0.0.1");
        assert_eq!(peer_ip("[::1]:9"), "::1");
        assert_eq!(peer_ip("not an address"), "not an address");
    }
}
