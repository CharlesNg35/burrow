use std::cell::RefCell;
use std::collections::HashMap;

use burrow_admin_logic::presence::Presence;
use burrow_admin_proto::schema::Entity;
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::status::{ClientStatus, ServiceStatus, VisitorStatus};
use burrow_config::entity_of_section;

use crate::route::Route;
use crate::screens::Row;
use crate::screens::paths::clients_of;

use super::index::Index;
use super::rows::{client_row, service_row, visitor_row};

#[derive(PartialEq)]
enum Live {
    Service(Option<ServiceStatus>),
    Client(Option<ClientStatus>),
    Visitor(Option<VisitorStatus>),
}

impl Live {
    fn same(&self, kind: Option<&Entity>, ix: &Index, name: &str) -> bool {
        match (self, kind) {
            (Live::Client(c), Some(Entity::Client)) => c.as_ref() == ix.client(name),
            (Live::Visitor(c), Some(Entity::Visitor)) => c.as_ref() == ix.visitor(name),
            (Live::Service(c), None | Some(Entity::Service)) => c.as_ref() == ix.service(name),
            _ => false,
        }
    }

    fn now(kind: Option<&Entity>, ix: &Index, name: &str) -> Live {
        match kind {
            Some(Entity::Client) => Live::Client(ix.client(name).cloned()),
            Some(Entity::Visitor) => Live::Visitor(ix.visitor(name).cloned()),
            _ => Live::Service(ix.service(name).cloned()),
        }
    }
}

pub(super) struct Ask<'a> {
    pub section: &'a str,
    pub name: &'a str,
    pub v: &'a Value,
    pub https_on: bool,
    pub me: Option<&'a str>,
    pub pending: Option<&'static str>,
}

struct Kept {
    v: Value,
    https_on: bool,
    me: Option<String>,
    pending: Option<&'static str>,
    known: bool,
    live: Live,
    clients: Vec<String>,
    presence: Vec<(Presence, bool)>,
    linked: Vec<String>,
    row: Row,
}

thread_local! {
    static KEPT: RefCell<HashMap<(String, String), Kept>> = RefCell::new(HashMap::new());
}

fn linked(kind: Option<&Entity>, ix: &Index, name: &str) -> Vec<String> {
    match kind {
        Some(Entity::Client) => ix.served(name).to_vec(),
        Some(Entity::Visitor) => ix.may_use(name).to_vec(),
        _ => Vec::new(),
    }
}

fn fresh(a: &Ask, kind: Option<&Entity>, ix: &Index) -> Row {
    let to = Route::Detail(a.section.to_string(), a.name.to_string());
    match kind {
        Some(Entity::Client) => client_row(a.name, ix, a.pending, to),
        Some(Entity::Visitor) => visitor_row(a.name, a.v, ix, a.me, a.pending, to),
        _ => service_row(a.name, (a.v, a.https_on), ix, a.pending, to),
    }
}

fn still_good(k: &Kept, a: &Ask, kind: Option<&Entity>, ix: &Index) -> bool {
    k.v == *a.v
        && k.https_on == a.https_on
        && k.me.as_deref() == a.me
        && k.pending == a.pending
        && k.known == ix.status.is_some()
        && k.live.same(kind, ix, a.name)
        && k.clients.len() == k.presence.len()
        && k.clients
            .iter()
            .zip(&k.presence)
            .all(|(c, p)| (ix.presence(c), ix.connected(c)) == *p)
        && linked_same(&k.linked, kind, ix, a.name)
}

fn linked_same(was: &[String], kind: Option<&Entity>, ix: &Index, name: &str) -> bool {
    match kind {
        Some(Entity::Client) => was == ix.served(name),
        Some(Entity::Visitor) => was == ix.may_use(name),
        _ => true,
    }
}

pub(super) fn row(a: &Ask, ix: &Index) -> Row {
    let kind = entity_of_section(a.section);
    let kind = kind.as_ref();
    let key = (a.section.to_string(), a.name.to_string());
    if let Some(row) = KEPT.with(|k| {
        k.borrow()
            .get(&key)
            .filter(|k| still_good(k, a, kind, ix))
            .map(|k| k.row.clone())
    }) {
        return row;
    }
    let row = fresh(a, kind, ix);
    let clients = if matches!(kind, None | Some(Entity::Service)) {
        clients_of(a.v)
    } else {
        Vec::new()
    };
    let kept = Kept {
        v: a.v.clone(),
        https_on: a.https_on,
        me: a.me.map(str::to_string),
        pending: a.pending,
        known: ix.status.is_some(),
        live: Live::now(kind, ix, a.name),
        presence: clients
            .iter()
            .map(|c| (ix.presence(c), ix.connected(c)))
            .collect(),
        clients,
        linked: linked(kind, ix, a.name),
        row: row.clone(),
    };
    KEPT.with(|k| k.borrow_mut().insert(key, kept));
    row
}

pub(super) fn keep_only(section: &str, names: &dyn Fn(&str) -> bool) {
    KEPT.with(|k| {
        k.borrow_mut().retain(|(s, n), _| s != section || names(n));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::serde_json::json;
    use burrow_admin_proto::status::Status;

    fn ask<'a>(v: &'a Value) -> Ask<'a> {
        Ask {
            section: "services",
            name: "web",
            v,
            https_on: false,
            me: None,
            pending: None,
        }
    }

    #[test]
    fn a_row_is_built_again_only_when_its_inputs_change() {
        let v = json!({"client": "home", "target": 80});
        let draft = json!({"services": {"web": v.clone()}});
        let mut st = Status::default();
        st.services.push(ServiceStatus {
            name: "web".into(),
            open: 1,
            ..ServiceStatus::default()
        });
        let ix = Index::new(Some(&st), &draft, super::super::file::Files::default());
        let first = row(&ask(&v), &ix);
        assert_eq!(first.meta, "1 open");
        let kept = KEPT.with(|k| {
            k.borrow()
                .get(&("services".into(), "web".into()))
                .map(|k| still_good(k, &ask(&v), None, &ix))
        });
        assert_eq!(kept, Some(true));
        let mut busy = st.clone();
        busy.services[0].open = 2;
        let ix = Index::new(Some(&busy), &draft, super::super::file::Files::default());
        assert_eq!(row(&ask(&v), &ix).meta, "2 open");
        let other = json!({"client": "home", "target": 81});
        assert!(row(&ask(&other), &ix).line2.contains("81"));
        keep_only("services", &|_| false);
        assert!(KEPT.with(|k| k.borrow().is_empty()));
    }
}
