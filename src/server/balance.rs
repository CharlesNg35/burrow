use std::collections::{BTreeMap, HashMap};

use crate::config::settings::Service;

#[derive(Default)]
pub(super) struct Wheels(HashMap<String, Wheel>);

#[derive(Default)]
struct Wheel {
    config: Vec<(String, u32)>,
    current: HashMap<String, i64>,
}

impl Wheel {
    fn fits(&self, svc: &Service) -> bool {
        self.config.len() == svc.clients.len()
            && self
                .config
                .iter()
                .zip(&svc.clients)
                .all(|((n, w), c)| n == c && *w == svc.weight_of(c))
    }

    fn of(svc: &Service) -> Wheel {
        Wheel {
            config: svc
                .clients
                .iter()
                .map(|c| (c.clone(), svc.weight_of(c)))
                .collect(),
            current: HashMap::new(),
        }
    }
}

impl Wheels {
    pub(super) fn turn(&mut self, service: &str, svc: &Service, group: &[(&str, u32)]) -> usize {
        let wheel = match self.0.get_mut(service) {
            Some(w) if w.fits(svc) => w,
            _ => self
                .0
                .entry(service.to_string())
                .insert_entry(Wheel::of(svc))
                .into_mut(),
        };
        smooth(&mut wheel.current, group)
    }

    pub(super) fn retain(&mut self, services: &BTreeMap<String, Service>) {
        self.0.retain(|name, _| services.contains_key(name));
    }
}

fn smooth(current: &mut HashMap<String, i64>, group: &[(&str, u32)]) -> usize {
    let total: i64 = group.iter().map(|(_, w)| i64::from(*w)).sum();
    let mut best: Option<(usize, i64)> = None;
    for (i, (name, weight)) in group.iter().enumerate() {
        if !current.contains_key(*name) {
            current.insert(name.to_string(), 0);
        }
        let Some(now) = current.get_mut(*name) else {
            continue;
        };
        *now += i64::from(*weight);
        if best.is_none_or(|(_, b)| *now > b) {
            best = Some((i, *now));
        }
    }
    let Some((i, _)) = best else {
        return 0;
    };
    if let Some(c) = current.get_mut(group[i].0) {
        *c -= total;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(group: &[(&str, u32)], turns: usize, current: &mut HashMap<String, i64>) -> String {
        (0..turns)
            .map(|_| group[smooth(current, group)].0)
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
            weights: [("a".to_string(), w)]
                .into_iter()
                .filter(|(_, w)| *w != 1)
                .collect(),
            ..Service::default()
        };
        let mut wheels = Wheels::default();
        let group = [("a", 1), ("b", 1)];
        assert_eq!(wheels.turn("web", &svc(1), &group), 0);
        assert_eq!(wheels.turn("web", &svc(1), &group), 1);
        let heavy = [("a", 2), ("b", 1)];
        assert_eq!(wheels.turn("web", &svc(2), &heavy), 0);
        assert_eq!(wheels.0["web"].config[0], ("a".to_string(), 2));
        wheels.retain(&BTreeMap::new());
        assert!(wheels.0.is_empty());
    }
}
