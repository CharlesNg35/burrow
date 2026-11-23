use std::cell::RefCell;
use std::collections::HashMap;
use std::thread::LocalKey;

use burrow_admin_proto::status::{ServiceStatus, Status, VisitorStatus};

const SCAN: usize = 32;

#[derive(Default)]
struct Index {
    at: usize,
    len: usize,
    by: HashMap<String, usize>,
}

thread_local! {
    static SERVICES: RefCell<Index> = RefCell::new(Index::default());
    static VISITORS: RefCell<Index> = RefCell::new(Index::default());
}

fn rebuild<T>(ix: &mut Index, list: &[T], key: fn(&T) -> &str) {
    ix.at = list.as_ptr() as usize;
    ix.len = list.len();
    ix.by = list
        .iter()
        .enumerate()
        .map(|(i, x)| (key(x).to_string(), i))
        .collect();
}

fn find<'a, T>(
    cell: &'static LocalKey<RefCell<Index>>,
    list: &'a [T],
    name: &str,
    key: fn(&T) -> &str,
) -> Option<&'a T> {
    if list.len() <= SCAN {
        return list.iter().find(|x| key(x) == name);
    }
    cell.with(|c| {
        let mut ix = c.borrow_mut();
        let mut fresh = false;
        if ix.at != list.as_ptr() as usize || ix.len != list.len() {
            rebuild(&mut ix, list, key);
            fresh = true;
        }
        let hit = |ix: &Index| {
            ix.by
                .get(name)
                .and_then(|&i| list.get(i))
                .filter(|x| key(x) == name)
        };
        match hit(&ix) {
            Some(x) => Some(x),
            None if fresh => None,
            None => {
                rebuild(&mut ix, list, key);
                hit(&ix)
            }
        }
    })
}

pub fn service<'a>(status: &'a Status, name: &str) -> Option<&'a ServiceStatus> {
    find(&SERVICES, &status.services, name, |x| x.name.as_str())
}

pub fn visitor<'a>(status: &'a Status, name: &str) -> Option<&'a VisitorStatus> {
    find(&VISITORS, &status.visitors, name, |x| x.name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(n: usize) -> Status {
        Status {
            services: (0..n)
                .map(|i| ServiceStatus {
                    name: format!("s{i}"),
                    ..ServiceStatus::default()
                })
                .collect(),
            ..Status::default()
        }
    }

    #[test]
    fn a_name_is_found_in_a_long_list_and_a_new_status_is_indexed_again() {
        let a = status(100);
        assert_eq!(service(&a, "s42").map(|s| s.name.as_str()), Some("s42"));
        assert!(service(&a, "nope").is_none());
        let b = status(5);
        assert_eq!(service(&b, "s3").map(|s| s.name.as_str()), Some("s3"));
        let mut c = status(100);
        c.services.swap(1, 2);
        assert_eq!(service(&c, "s1").map(|s| s.name.as_str()), Some("s1"));
        assert_eq!(service(&c, "s2").map(|s| s.name.as_str()), Some("s2"));
    }
}
