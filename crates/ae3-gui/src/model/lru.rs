//! A least-recently-used cache that accounts for the bytes of its values.
//!
//! The GUI keeps decoded textures (thumbnails and full-size previews) in one
//! of these. Evicted values are handed back to the caller, so dropping a
//! texture handle (which frees the GPU texture) happens where the caller
//! decides.

use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;

/// Bytes a decoded texture of `width` x `height` RGBA8 pixels occupies.
#[must_use]
pub fn rgba_bytes(width: usize, height: usize) -> u64 {
    (width as u64)
        .saturating_mul(height as u64)
        .saturating_mul(4)
}

struct Entry<V> {
    value: V,
    bytes: u64,
    tick: u64,
}

/// An LRU map whose capacity is a byte budget rather than an entry count.
pub struct ByteLru<K, V> {
    capacity: u64,
    used: u64,
    tick: u64,
    entries: HashMap<K, Entry<V>>,
    /// Use order: tick -> key; the first entry is the least recently used.
    order: BTreeMap<u64, K>,
}

impl<K: Hash + Eq + Clone, V> ByteLru<K, V> {
    #[must_use]
    pub fn new(capacity: u64) -> Self {
        Self {
            capacity,
            used: 0,
            tick: 0,
            entries: HashMap::new(),
            order: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn capacity(&self) -> u64 {
        self.capacity
    }

    /// Bytes held by every entry.
    #[must_use]
    pub fn used_bytes(&self) -> u64 {
        self.used
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[cfg(test)]
    #[must_use]
    pub fn contains(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    fn next_tick(&mut self) -> u64 {
        self.tick += 1;
        self.tick
    }

    /// The value for `key`, marking it as the most recently used.
    pub fn get(&mut self, key: &K) -> Option<&V> {
        let tick = self.next_tick();
        let entry = self.entries.get_mut(key)?;
        self.order.remove(&entry.tick);
        entry.tick = tick;
        self.order.insert(tick, key.clone());
        Some(&entry.value)
    }

    /// The value for `key` without changing its recency.
    #[cfg(test)]
    #[must_use]
    pub fn peek(&self, key: &K) -> Option<&V> {
        self.entries.get(key).map(|e| &e.value)
    }

    /// Inserts `value`, costing `bytes`, as the most recently used entry,
    /// and evicts least recently used entries until the cache fits its
    /// capacity again. A value larger than the whole capacity is still kept
    /// (alone), so the entry just asked for is always available. Returns the
    /// evicted entries, including a replaced value under the same key.
    pub fn insert(&mut self, key: K, value: V, bytes: u64) -> Vec<(K, V)> {
        let mut evicted = Vec::new();
        if let Some(old) = self.remove(&key) {
            evicted.push((key.clone(), old));
        }
        let tick = self.next_tick();
        self.used = self.used.saturating_add(bytes);
        self.order.insert(tick, key.clone());
        self.entries.insert(key, Entry { value, bytes, tick });
        while self.used > self.capacity && self.entries.len() > 1 {
            let Some((_, oldest)) = self.order.pop_first() else {
                break;
            };
            if let Some(entry) = self.entries.remove(&oldest) {
                self.used -= entry.bytes;
                evicted.push((oldest, entry.value));
            }
        }
        evicted
    }

    /// Removes `key`, returning its value.
    pub fn remove(&mut self, key: &K) -> Option<V> {
        let entry = self.entries.remove(key)?;
        self.order.remove(&entry.tick);
        self.used -= entry.bytes;
        Some(entry.value)
    }

    /// Removes every entry whose key fails `keep`, returning how many bytes
    /// were freed.
    pub fn retain(&mut self, mut keep: impl FnMut(&K) -> bool) -> u64 {
        let before = self.used;
        let gone: Vec<K> = self.entries.keys().filter(|k| !keep(k)).cloned().collect();
        for key in gone {
            self.remove(&key);
        }
        before - self.used
    }

    /// Removes everything.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.used = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_least_recently_used_by_bytes() {
        let mut lru = ByteLru::new(100);
        assert_eq!(lru.insert("a", 1, 40), vec![]);
        assert_eq!(lru.insert("b", 2, 40), vec![]);
        assert_eq!(lru.used_bytes(), 80);
        // Touch "a" so "b" is the oldest.
        assert_eq!(lru.get(&"a"), Some(&1));
        let evicted = lru.insert("c", 3, 40);
        assert_eq!(evicted, vec![("b", 2)]);
        assert_eq!(lru.used_bytes(), 80);
        assert!(lru.contains(&"a") && lru.contains(&"c"));
    }

    #[test]
    fn evicts_several_to_fit_a_large_entry() {
        let mut lru = ByteLru::new(100);
        for (i, k) in ["a", "b", "c", "d"].into_iter().enumerate() {
            lru.insert(k, i, 25);
        }
        assert_eq!(lru.used_bytes(), 100);
        let evicted = lru.insert("big", 9, 70);
        assert_eq!(evicted, vec![("a", 0), ("b", 1), ("c", 2)]);
        assert_eq!(lru.used_bytes(), 95);
        assert_eq!(lru.len(), 2);
    }

    #[test]
    fn keeps_an_oversized_entry_alone() {
        let mut lru = ByteLru::new(10);
        lru.insert("a", 1, 5);
        let evicted = lru.insert("huge", 2, 50);
        assert_eq!(evicted, vec![("a", 1)]);
        assert_eq!(lru.len(), 1);
        assert_eq!(lru.used_bytes(), 50);
        // The next insert evicts the oversized one.
        let evicted = lru.insert("b", 3, 5);
        assert_eq!(evicted, vec![("huge", 2)]);
        assert_eq!(lru.used_bytes(), 5);
    }

    #[test]
    fn replacing_a_key_returns_the_old_value_and_fixes_accounting() {
        let mut lru = ByteLru::new(100);
        lru.insert("a", 1, 30);
        let evicted = lru.insert("a", 2, 50);
        assert_eq!(evicted, vec![("a", 1)]);
        assert_eq!(lru.used_bytes(), 50);
        assert_eq!(lru.peek(&"a"), Some(&2));
        assert_eq!(lru.len(), 1);
    }

    #[test]
    fn remove_retain_and_clear_free_bytes() {
        let mut lru = ByteLru::new(1000);
        lru.insert(1, "x", 10);
        lru.insert(2, "y", 20);
        lru.insert(3, "z", 30);
        assert_eq!(lru.remove(&2), Some("y"));
        assert_eq!(lru.used_bytes(), 40);
        assert_eq!(lru.retain(|k| *k != 3), 30);
        assert_eq!(lru.used_bytes(), 10);
        lru.clear();
        assert!(lru.is_empty());
        assert_eq!(lru.used_bytes(), 0);
        // Order bookkeeping is consistent after removals.
        lru.insert(4, "w", 600);
        lru.insert(5, "v", 600);
        assert!(!lru.contains(&4));
    }

    #[test]
    fn peek_does_not_change_recency() {
        let mut lru = ByteLru::new(20);
        lru.insert("a", 1, 10);
        lru.insert("b", 2, 10);
        assert_eq!(lru.peek(&"a"), Some(&1));
        let evicted = lru.insert("c", 3, 10);
        assert_eq!(evicted, vec![("a", 1)]);
    }

    #[test]
    fn rgba_bytes_saturates() {
        assert_eq!(rgba_bytes(128, 64), 128 * 64 * 4);
        assert_eq!(rgba_bytes(usize::MAX, usize::MAX), u64::MAX);
    }
}
