use std::collections::HashMap;
use std::ops::Deref;
use std::sync::Arc;

use burrow_admin_proto::delta::Row;
use burrow_admin_proto::status::{ClientStatus, ServiceStatus, Status, VisitorStatus};

type Names = Arc<HashMap<String, usize>>;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Live {
    status: Status,
    services: Names,
    clients: Names,
    visitors: Names,
}

fn same_keys<T: Row>(a: &[T], b: &[T]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.key() == y.key())
}

fn names<T: Row>(list: &[T], before: Option<(&[T], &Names)>) -> Names {
    if let Some((old, names)) = before
        && same_keys(old, list)
    {
        return names.clone();
    }
    Arc::new(
        list.iter()
            .enumerate()
            .rev()
            .map(|(i, x)| (x.key().to_string(), i))
            .collect(),
    )
}

fn find<'a, T: Row>(list: &'a [T], names: &Names, name: &str) -> Option<&'a T> {
    names.get(name).and_then(|&i| list.get(i))
}

impl Live {
    pub fn new(status: Status, before: Option<&Live>) -> Live {
        Live {
            services: names(
                &status.services,
                before.map(|b| (b.status.services.as_slice(), &b.services)),
            ),
            clients: names(
                &status.clients,
                before.map(|b| (b.status.clients.as_slice(), &b.clients)),
            ),
            visitors: names(
                &status.visitors,
                before.map(|b| (b.status.visitors.as_slice(), &b.visitors)),
            ),
            status,
        }
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    pub fn service(&self, name: &str) -> Option<&ServiceStatus> {
        find(&self.status.services, &self.services, name)
    }

    pub fn client(&self, name: &str) -> Option<&ClientStatus> {
        find(&self.status.clients, &self.clients, name)
    }

    pub fn visitor(&self, name: &str) -> Option<&VisitorStatus> {
        find(&self.status.visitors, &self.visitors, name)
    }
}

impl From<Status> for Live {
    fn from(status: Status) -> Live {
        Live::new(status, None)
    }
}

impl Deref for Live {
    type Target = Status;

    fn deref(&self) -> &Status {
        &self.status
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(names: &[&str]) -> Status {
        Status {
            services: names
                .iter()
                .map(|n| ServiceStatus {
                    name: n.to_string(),
                    ..ServiceStatus::default()
                })
                .collect(),
            clients: vec![ClientStatus {
                name: "home".into(),
                ..ClientStatus::default()
            }],
            ..Status::default()
        }
    }

    #[test]
    fn names_are_found_and_the_index_is_kept_while_the_names_stay() {
        let a = Live::from(status(&["a", "b", "c"]));
        assert_eq!(a.service("b").map(|s| s.name.as_str()), Some("b"));
        assert!(a.service("z").is_none() && a.visitor("a").is_none());
        assert_eq!(a.client("home").map(|c| c.name.as_str()), Some("home"));
        let mut next = status(&["a", "b", "c"]);
        next.services[1].open = 4;
        let b = Live::new(next, Some(&a));
        assert!(
            Arc::ptr_eq(&a.services, &b.services),
            "same names, same index"
        );
        assert_eq!(b.service("b").map(|s| s.open), Some(4));
        let c = Live::new(status(&["c", "a"]), Some(&b));
        assert!(!Arc::ptr_eq(&b.services, &c.services));
        assert_eq!(c.service("a").map(|s| s.name.as_str()), Some("a"));
        assert!(c.service("b").is_none());
        let d = Live::from(status(&["x", "x"]));
        assert_eq!(
            d.services.get("x"),
            Some(&0),
            "the first of a repeated name"
        );
        assert_eq!(d.uptime_secs, 0, "reads as the status");
    }
}
