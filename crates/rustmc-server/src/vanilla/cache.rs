//! Fixed-capacity coordinate memos for the vanilla generator.
//!
//! Every cache in this module maps world coordinates to a value that is a
//! pure function of those coordinates and the world seed: a chunk's carve
//! mask, a column's surface top, an aquifer cell's center. Dropping an
//! entry can therefore only cost a recomputation, never change a generated
//! block, which is what makes eviction safe here and lets each owner pick a
//! capacity from its access pattern instead of keeping everything forever.
//!
//! The policy is two-generation FIFO with promotion, the shape of the
//! classic 2Q double queue and the tiering Caffeine uses behind its TinyLFU
//! window: a young generation accepts new entries, and when it reaches
//! `capacity` the older generation is dropped wholesale and the young one
//! becomes the old. A hit in the older generation moves the entry back to
//! the young one, so a working set that fits in `capacity` is never evicted
//! while a one-pass sweep of everything else flows past it. Total entries
//! never exceed `2 * capacity` and no allocation grows with the size of the
//! explored world.
//!
//! This is deliberately not a general-purpose crate dependency: the caches
//! need no TTL, no statistics, no concurrent access (each generator is used
//! from one thread behind its own `RefCell`), and the whole policy is the
//! six lines above. See the capacity comments on `VanillaGenerator` and
//! `NoiseBasedAquifer` for the per-cache reasoning.

use rustc_hash::FxHashMap;
use std::hash::Hash;

/// A hash map bounded to `2 * capacity` entries by two-generation FIFO
/// eviction with promotion on old-generation hits.
pub(crate) struct BoundedCache<K, V> {
    capacity: usize,
    young: FxHashMap<K, V>,
    old: FxHashMap<K, V>,
}

impl<K: Eq + Hash + Clone, V> BoundedCache<K, V> {
    /// Creates an empty cache holding at most `2 * capacity` entries. A
    /// capacity of zero is raised to one: an unbounded-by-accident cache
    /// would defeat the purpose of the type.
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            capacity,
            young: FxHashMap::with_capacity_and_hasher(capacity, Default::default()),
            old: FxHashMap::with_capacity_and_hasher(capacity, Default::default()),
        }
    }

    /// Looks a key up for reading or in-place update, aging a hit in the
    /// older generation back into the young one so a reused entry outlives
    /// the sweep that touched it.
    ///
    /// All mutation happens before any reference escapes, because the only
    /// borrow that may outlive this call is the final lookup: a young hit
    /// therefore costs one extra `contains_key` probe, the price of
    /// expressing the promotion without the unstable `raw_entry_mut`.
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        if !self.young.contains_key(key)
            && let Some(value) = self.old.remove(key)
        {
            if self.young.len() >= self.capacity {
                self.rotate();
            }
            self.young.insert(key.clone(), value);
        }
        self.young.get_mut(key)
    }

    /// Inserts a value, evicting the oldest generation when the young one
    /// is full. Re-inserting a live key replaces its value in place.
    pub fn insert(&mut self, key: K, value: V) {
        if !self.young.contains_key(&key) {
            self.old.remove(&key);
            if self.young.len() >= self.capacity {
                self.rotate();
            }
        }
        self.young.insert(key, value);
    }

    /// Entry count across both generations, for occupancy assertions.
    pub fn entries(&self) -> usize {
        self.young.len() + self.old.len()
    }

    fn rotate(&mut self) {
        rotate(&mut self.young, &mut self.old);
    }
}

/// Ages the young generation into the old, dropping what was old. Swap
/// plus `clear` keeps both allocations for reuse instead of reallocating
/// the young map on every rotation.
fn rotate<K, V>(young: &mut FxHashMap<K, V>, old: &mut FxHashMap<K, V>) {
    std::mem::swap(young, old);
    young.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_holds_more_than_double_capacity() {
        let mut cache = BoundedCache::<u64, u64>::new(8);
        for key in 0..10_000 {
            cache.insert(key, key * 2);
            assert!(cache.entries() <= 16, "entries {}", cache.entries());
        }
        // The most recent keys are still there; everything older flowed
        // through and out.
        assert_eq!(cache.get_mut(&9_999).cloned(), Some(19_998));
        assert_eq!(cache.get_mut(&0).cloned(), None);
    }

    #[test]
    fn promotion_lets_a_hot_key_outlive_a_cold_sweep() {
        let mut cache = BoundedCache::<u32, u32>::new(2);
        cache.insert(1, 10);
        cache.insert(2, 20);
        // Third insert ages the first generation out.
        cache.insert(3, 30);
        assert_eq!(cache.get_mut(&1).cloned(), Some(10));
        assert_eq!(cache.get_mut(&2).cloned(), Some(20));
        assert_eq!(cache.get_mut(&3).cloned(), Some(30));
        // Both promoted entries now sit in the young generation; the next
        // rotation drops only the aged-out one.
        cache.insert(4, 40);
        assert_eq!(
            cache.get_mut(&2).cloned(),
            Some(20),
            "a promoted entry survives"
        );
        assert_eq!(
            cache.get_mut(&1).cloned(),
            None,
            "an untouched entry is evicted"
        );
    }

    #[test]
    fn reinserting_a_live_key_replaces_its_value() {
        let mut cache = BoundedCache::<u32, u32>::new(4);
        cache.insert(7, 1);
        cache.insert(7, 2);
        assert_eq!(cache.entries(), 1);
        assert_eq!(cache.get_mut(&7).cloned(), Some(2));
        // Replacing must not rotate the generations either.
        for key in 0..4 {
            cache.insert(key, key);
        }
        let before = cache.entries();
        cache.insert(7, 3);
        assert_eq!(cache.entries(), before);
        assert_eq!(cache.get_mut(&7).cloned(), Some(3));
    }

    #[test]
    fn zero_capacity_is_raised_to_one() {
        let mut cache = BoundedCache::<u32, u32>::new(0);
        cache.insert(1, 1);
        cache.insert(2, 2);
        assert!(cache.entries() <= 2);
        assert_eq!(
            cache.get_mut(&2).cloned(),
            Some(2),
            "the newest entry survives"
        );
    }
}
