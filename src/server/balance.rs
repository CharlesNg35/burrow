use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::CountGuard;
use crate::config::settings::{EntryKey, Service};

#[derive(Default)]
pub(super) struct EntryCounts(Mutex<HashMap<String, HashMap<EntryKey, Arc<AtomicUsize>>>>);

impl EntryCounts {
    pub(super) fn enter(&self, service: &str, key: &EntryKey) -> CountGuard {
        let mut all = self.0.lock().expect("entry counts lock");
        let of = match all.get_mut(service) {
            Some(of) => of,
            None => all.entry(service.to_string()).or_default(),
        };
        super::enter_in(of, key).0
    }

    pub(super) fn current(&self, service: &str, key: &EntryKey) -> u64 {
        self.0
            .lock()
            .expect("entry counts lock")
            .get(service)
            .map_or(0, |of| super::current_in(of, key))
    }

    pub(super) fn prune(&self, services: &BTreeMap<String, Service>) {
        let mut all = self.0.lock().expect("entry counts lock");
        all.retain(|name, of| {
            let entries = services.get(name).map(|s| s.entries());
            of.retain(|key, c| {
                c.load(Ordering::Acquire) > 0
                    || entries
                        .as_ref()
                        .is_some_and(|e| e.iter().any(|e| &e.key == key))
            });
            !of.is_empty()
        });
    }
}

#[derive(Default)]
pub(super) struct Wheels(HashMap<String, Wheel>);

struct Wheel {
    id: u64,
    current: HashMap<EntryKey, i64>,
}

impl Wheels {
    pub(super) fn turn<'a>(
        &mut self,
        service: &str,
        id: u64,
        group: impl Iterator<Item = (&'a EntryKey, u32)>,
    ) -> usize {
        let wheel = match self.0.get_mut(service) {
            Some(w) if w.id == id => w,
            _ => self
                .0
                .entry(service.to_string())
                .insert_entry(Wheel {
                    id,
                    current: HashMap::new(),
                })
                .into_mut(),
        };
        smooth(&mut wheel.current, group)
    }

    pub(super) fn retain(&mut self, services: &BTreeMap<String, Service>) {
        self.0.retain(|name, _| services.contains_key(name));
    }
}

fn smooth<'a>(
    current: &mut HashMap<EntryKey, i64>,
    group: impl Iterator<Item = (&'a EntryKey, u32)>,
) -> usize {
    let mut total = 0i64;
    let mut best: Option<(usize, i64, &EntryKey)> = None;
    for (i, (key, weight)) in group.enumerate() {
        total += i64::from(weight);
        let now = match current.get_mut(key) {
            Some(c) => {
                *c += i64::from(weight);
                *c
            }
            None => {
                current.insert(key.clone(), i64::from(weight));
                i64::from(weight)
            }
        };
        if best.is_none_or(|(_, b, _)| now > b) {
            best = Some((i, now, key));
        }
    }
    let Some((i, _, key)) = best else {
        return 0;
    };
    if let Some(c) = current.get_mut(key) {
        *c -= total;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings::Entry;

    fn keys(group: &[(&str, u32)]) -> Vec<(EntryKey, u32)> {
        group
            .iter()
            .map(|(n, w)| (Entry::new(n, None, *w, "t:1").key, *w))
            .collect()
    }

    fn run(group: &[(&str, u32)], turns: usize, current: &mut HashMap<EntryKey, i64>) -> String {
        let k = keys(group);
        (0..turns)
            .map(|_| group[smooth(current, k.iter().map(|(k, w)| (k, *w)))].0)
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn weights_5_1_1_give_the_nginx_sequence() {
        let group = [("a", 5), ("b", 1), ("c", 1)];
        let mut current = HashMap::new();
        assert_eq!(run(&group, 7, &mut current), "a a b a c a a");
        assert_eq!(run(&group, 7, &mut current), "a a b a c a a");
    }

    #[test]
    fn even_weights_take_turns_in_list_order() {
        let group = [("a", 1), ("b", 1), ("c", 1)];
        assert_eq!(run(&group, 6, &mut HashMap::new()), "a b c a b c");
    }

    #[test]
    fn a_client_left_out_keeps_the_ratio_of_the_others() {
        let mut current = HashMap::new();
        run(&[("a", 3), ("b", 1), ("c", 2)], 5, &mut current);
        let two = [("a", 3), ("b", 1)];
        let seq = run(&two, 40, &mut current);
        let a = seq.split(' ').filter(|n| *n == "a").count();
        assert!((29..=31).contains(&a), "{seq}");
        assert!(current.values().all(|c| c.abs() <= 12), "{current:?}");
    }

    #[test]
    fn a_changed_weight_starts_a_new_wheel_and_removed_services_go() {
        let svc = |w: u32| Service {
            clients: vec!["a".into(), "b".into()],
            entries: vec![
                Entry::new("a", None, w, "t:1"),
                Entry::new("b", None, 1, "t:1"),
            ],
            ..Service::default()
        };
        let mut wheels = Wheels::default();
        let pairs = |s: &Service| -> Vec<(EntryKey, u32)> {
            s.entries
                .iter()
                .map(|e| (e.key.clone(), e.weight))
                .collect()
        };
        let (even, heavy) = (svc(1), svc(2));
        let (e, h) = (pairs(&even), pairs(&heavy));
        let turn = |w: &mut Wheels, s: &Service, p: &[(EntryKey, u32)]| {
            w.turn("web", s.entries_id(), p.iter().map(|(k, x)| (k, *x)))
        };
        assert_eq!(turn(&mut wheels, &even, &e), 0);
        assert_eq!(turn(&mut wheels, &even, &e), 1);
        assert_eq!(turn(&mut wheels, &heavy, &h), 0);
        assert_eq!(wheels.0["web"].id, heavy.entries_id());
        assert_ne!(even.entries_id(), heavy.entries_id());
        wheels.retain(&BTreeMap::new());
        assert!(wheels.0.is_empty());
    }

    #[test]
    fn entry_counts_rise_per_enter_and_fall_back_to_zero_when_the_guards_drop() {
        let counts = EntryCounts::default();
        let key = Entry::new("a", None, 1, "t:1").key;
        let first = counts.enter("web", &key);
        let second = counts.enter("web", &key);
        assert_eq!(counts.current("web", &key), 2);
        assert_eq!(counts.current("other", &key), 0);
        drop(first);
        drop(second);
        assert_eq!(counts.current("web", &key), 0);
        assert_eq!(counts.0.lock().unwrap()["web"].len(), 1);
    }
}
