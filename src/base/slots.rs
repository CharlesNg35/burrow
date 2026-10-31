use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::Hash;

pub fn shared_slot<K, Q, V>(map: &mut HashMap<K, V>, key: &Q, make: impl FnOnce() -> V) -> V
where
    K: Borrow<Q> + Eq + Hash,
    Q: Eq + Hash + ToOwned<Owned = K> + ?Sized,
    V: Clone,
{
    if let Some(v) = map.get(key) {
        return v.clone();
    }
    map.entry(key.to_owned()).or_insert_with(make).clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_slot_is_made_once_and_shared_after() {
        let mut map: HashMap<String, Arc<u8>> = HashMap::new();
        let first = shared_slot(&mut map, "a", || Arc::new(1));
        let again = shared_slot(&mut map, "a", || Arc::new(2));
        assert!(Arc::ptr_eq(&first, &again));
        assert_eq!(*again, 1);
        assert_eq!(map.len(), 1);
    }
}
