use burrow_admin_logic::live::Live;
use std::collections::HashMap;

use burrow_admin_logic::presence::Presence;
use burrow_admin_proto::schema::Entity;
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::status::{ClientStatus, ServiceStatus, VisitorStatus};
use burrow_config::{Entry, ServerConfig, ServiceEntry, WebConfig};

use super::file::Files;
use crate::screens::paths::clients_of;

pub(crate) struct Index<'a> {
    pub status: Option<&'a Live>,
    pub files: Files,
    pub https_on: bool,
    reserved: Vec<String>,
    served: HashMap<String, Vec<String>>,
    may: HashMap<String, Vec<String>>,
}

fn push_once(map: &mut HashMap<String, Vec<String>>, key: String, name: &str) {
    let list = map.entry(key).or_default();
    if list.last().map(String::as_str) != Some(name) {
        list.push(name.to_string());
    }
}

impl<'a> Index<'a> {
    pub fn new(section: &str, status: Option<&'a Live>, draft: &Value, files: Files) -> Index<'a> {
        let kind = burrow_config::entity_of_section(section);
        let (clients, visitors) = (
            matches!(kind, Some(Entity::Client)),
            matches!(kind, Some(Entity::Visitor)),
        );
        let mut served = HashMap::new();
        let mut may = HashMap::new();
        for (n, svc) in ServiceEntry::entries(draft).into_iter().flatten() {
            if clients {
                for c in clients_of(svc) {
                    push_once(&mut served, c, n);
                }
            }
            if visitors {
                let list = ServiceEntry::VISITORS.get(svc).and_then(Value::as_array);
                for v in list.into_iter().flatten().filter_map(Value::as_str) {
                    push_once(&mut may, v.to_string(), n);
                }
            }
        }
        Index {
            status,
            files,
            https_on: ServerConfig::WEB
                .get(draft)
                .and_then(|w| WebConfig::HTTPS.get(w))
                .is_some_and(|h| !h.is_null()),
            reserved: crate::format::reserved_clients(),
            served,
            may,
        }
    }

    pub fn service(&self, name: &str) -> Option<&'a ServiceStatus> {
        self.status.and_then(|s| s.service(name))
    }

    pub fn client(&self, name: &str) -> Option<&'a ClientStatus> {
        self.status.and_then(|s| s.client(name))
    }

    pub fn visitor(&self, name: &str) -> Option<&'a VisitorStatus> {
        self.status.and_then(|s| s.visitor(name))
    }

    pub fn presence(&self, name: &str) -> Presence {
        if self.reserved.iter().any(|r| r == name) {
            return Presence::Here;
        }
        match (self.status, self.client(name)) {
            (None, _) => Presence::Unknown,
            (Some(_), Some(c)) if c.local => Presence::Here,
            (Some(_), Some(c)) if c.connected => Presence::Online,
            _ => Presence::Offline,
        }
    }

    pub fn connected(&self, name: &str) -> bool {
        self.client(name).is_some_and(|c| c.local || c.connected)
    }

    pub fn served(&self, client: &str) -> &[String] {
        self.served.get(client).map_or(&[], Vec::as_slice)
    }

    pub fn may_use(&self, visitor: &str) -> &[String] {
        self.may.get(visitor).map_or(&[], Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::serde_json::json;

    #[test]
    fn the_index_lists_what_each_client_serves_and_each_visitor_may_use_once() {
        let draft = json!({"services": {
            "a": {"clients": ["home", {"name": "home", "target": "127.0.0.1:9"}], "target": 1, "visitors": ["laptop"]},
            "b": {"client": "home", "target": 2, "visitors": ["laptop", "phone"]},
        }});
        let i = Index::new("clients", None, &draft, Files::default());
        assert_eq!(i.served("home"), ["a", "b"]);
        assert!(
            i.may_use("laptop").is_empty(),
            "the clients list needs no visitor links"
        );
        let i = Index::new("visitors", None, &draft, Files::default());
        assert_eq!(i.may_use("laptop"), ["a", "b"]);
        assert_eq!(i.may_use("phone"), ["b"]);
        assert!(i.served("office").is_empty());
        assert_eq!(i.presence("home"), Presence::Unknown);
    }
}
