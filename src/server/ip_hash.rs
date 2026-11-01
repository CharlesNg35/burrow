//! `mode: ip_hash`: each visitor address ranks a service's entries by weighted rendezvous hashing.

use std::cmp::Reverse;
use std::net::IpAddr;

use aws_lc_rs::digest::{Context, SHA256};
use burrow_syntax::share::subnet_key;

use super::pick::Hop;
use crate::config::settings::EntryKey;

pub(super) fn rank(hops: &mut [Hop], from: IpAddr) {
    let visitor = visitor_bytes(from);
    hops.sort_by_cached_key(|h| Reverse(score(&h.key, h.weight, &visitor).to_bits()));
}

fn visitor_bytes(ip: IpAddr) -> Vec<u8> {
    match subnet_key(ip) {
        IpAddr::V4(v4) => v4.octets().to_vec(),
        IpAddr::V6(v6) => v6.octets()[..8].to_vec(),
    }
}

fn unit(key: &EntryKey, visitor: &[u8]) -> f64 {
    let mut h = Context::new(&SHA256);
    for part in [key.client.as_bytes(), key.base.as_bytes(), visitor] {
        h.update(&(part.len() as u64).to_be_bytes());
        h.update(part);
    }
    let digest = h.finish();
    let mut first = [0u8; 8];
    first.copy_from_slice(&digest.as_ref()[..8]);
    let bits = u64::from_be_bytes(first) >> 11;
    (bits as f64 + 0.5) / (1u64 << 53) as f64
}

fn score(key: &EntryKey, weight: u32, visitor: &[u8]) -> f64 {
    f64::from(weight.max(1)) / -unit(key, visitor).ln()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::net::Ipv4Addr;

    use super::*;
    use crate::config::settings::Entry;

    fn entries(group: &[(&str, u32)]) -> Vec<(EntryKey, u32)> {
        group
            .iter()
            .map(|(n, w)| (Entry::new(n, None, *w, "t:1").key, *w))
            .collect()
    }

    fn first(group: &[(EntryKey, u32)], ip: IpAddr) -> String {
        let v = visitor_bytes(ip);
        let (key, _) = group
            .iter()
            .max_by(|a, b| score(&a.0, a.1, &v).total_cmp(&score(&b.0, b.1, &v)))
            .unwrap();
        key.client.to_string()
    }

    fn addrs(n: u32) -> impl Iterator<Item = IpAddr> {
        (0..n).map(|i| IpAddr::V4(Ipv4Addr::from(0xC633_6400 + i * 7919)))
    }

    fn spread(group: &[(&str, u32)], n: u32) -> BTreeMap<String, u32> {
        let g = entries(group);
        let mut seen = BTreeMap::new();
        for ip in addrs(n) {
            *seen.entry(first(&g, ip)).or_insert(0) += 1;
        }
        seen
    }

    #[test]
    fn addresses_spread_over_the_clients_by_weight() {
        let even = spread(&[("a", 1), ("b", 1), ("c", 1)], 9000);
        for (c, n) in &even {
            assert!((2800..=3200).contains(n), "{c}: {even:?}");
        }
        let heavy = spread(&[("a", 3), ("b", 1)], 8000);
        assert!((5800..=6200).contains(&heavy["a"]), "{heavy:?}");
    }

    #[test]
    fn the_same_address_always_gets_the_same_order() {
        let g = entries(&[("a", 1), ("b", 2), ("c", 1)]);
        for ip in addrs(200) {
            let once = first(&g, ip);
            assert!((0..5).all(|_| first(&g, ip) == once));
        }
    }

    #[test]
    fn removing_a_client_moves_only_its_own_visitors() {
        let three = entries(&[("a", 1), ("b", 2), ("c", 1)]);
        let two: Vec<_> = three
            .iter()
            .filter(|(k, _)| &*k.client != "b")
            .cloned()
            .collect();
        for ip in addrs(3000) {
            let before = first(&three, ip);
            let after = first(&two, ip);
            if before != "b" {
                assert_eq!(before, after, "{ip} moved off a client that stayed");
            }
        }
    }

    #[test]
    fn rank_orders_the_hops_and_the_next_one_is_the_second_choice() {
        let g = entries(&[("a", 1), ("b", 1), ("c", 1)]);
        for ip in addrs(300) {
            let top = first(&g, ip);
            let rest: Vec<_> = g
                .iter()
                .filter(|(k, _)| *k.client != *top)
                .cloned()
                .collect();
            let second = first(&rest, ip);
            let v = visitor_bytes(ip);
            let mut scored: Vec<_> = g.iter().map(|(k, w)| (score(k, *w, &v), k)).collect();
            scored.sort_by(|x, y| y.0.total_cmp(&x.0));
            assert_eq!(*scored[0].1.client, *top);
            assert_eq!(*scored[1].1.client, *second);
        }
    }

    #[test]
    fn an_ipv6_visitor_keeps_its_client_within_its_64() {
        let g = entries(&[("a", 1), ("b", 1), ("c", 1), ("d", 1)]);
        let one: IpAddr = "2001:db8:1:2::1".parse().unwrap();
        let same: IpAddr = "2001:db8:1:2:ffff::9".parse().unwrap();
        assert_eq!(first(&g, one), first(&g, same));
        let mapped: IpAddr = "::ffff:198.51.100.7".parse().unwrap();
        let v4: IpAddr = "198.51.100.7".parse().unwrap();
        assert_eq!(first(&g, mapped), first(&g, v4));
    }

    #[test]
    fn a_target_of_its_own_is_its_own_entry() {
        let a1 = Entry::new("a", Some("x:1".into()), 1, "t:1").key;
        let a2 = Entry::new("a", Some("y:1".into()), 1, "t:1").key;
        let g = [(a1, 1), (a2, 1)];
        let mut seen = BTreeMap::new();
        for ip in addrs(2000) {
            let v = visitor_bytes(ip);
            let best = if score(&g[0].0, 1, &v) > score(&g[1].0, 1, &v) {
                0
            } else {
                1
            };
            *seen.entry(best).or_insert(0) += 1;
        }
        assert!(seen.values().all(|n| (900..=1100).contains(n)), "{seen:?}");
    }
}
