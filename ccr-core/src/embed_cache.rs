//! Content-addressed embedding cache for the BERT daemon.
//!
//! Dev loops are highly repetitive: the same warnings, file paths, and test
//! names appear run after run. Re-embedding them on every request is the
//! single largest avoidable cost in the daemon. This cache keys each text by
//! a 64-bit hash of `(text, normalize)` and stores its embedding, so a
//! re-run of `cargo test` after a one-line change only embeds the lines that
//! actually changed.
//!
//! Eviction is FIFO by insertion order: line frequency in tool output is
//! heavy-tailed, so the hot set (recurring warnings, paths) is re-inserted
//! quickly after eviction and precise LRU bookkeeping isn't worth the cost.

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};

/// Default max entries. 384 dims × 4 bytes ≈ 1.5 KB per entry,
/// so 20 000 entries ≈ 30 MB on top of the resident model.
pub const DEFAULT_CAPACITY: usize = 20_000;

/// Cache key: hash of the text plus the normalize flag (normalized and raw
/// embeddings of the same text are different vectors).
#[inline]
pub fn cache_key(text: &str, normalize: bool) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    normalize.hash(&mut h);
    h.finish()
}

pub struct EmbedCache {
    map: HashMap<u64, Vec<f32>>,
    order: VecDeque<u64>,
    capacity: usize,
    pub hits: u64,
    pub misses: u64,
}

impl EmbedCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            map: HashMap::with_capacity(capacity.min(4096)),
            order: VecDeque::with_capacity(capacity.min(4096)),
            capacity: capacity.max(1),
            hits: 0,
            misses: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Look up a single key, counting the hit/miss.
    pub fn get(&mut self, key: u64) -> Option<Vec<f32>> {
        match self.map.get(&key) {
            Some(emb) => {
                self.hits += 1;
                Some(emb.clone())
            }
            None => {
                self.misses += 1;
                None
            }
        }
    }

    /// Insert an embedding, evicting oldest entries when over capacity.
    pub fn insert(&mut self, key: u64, emb: Vec<f32>) {
        if self.map.insert(key, emb).is_none() {
            self.order.push_back(key);
        }
        while self.map.len() > self.capacity {
            match self.order.pop_front() {
                Some(old) => {
                    self.map.remove(&old);
                }
                None => break,
            }
        }
    }

    /// Batch lookup: returns one slot per text (`Some` on hit) plus the
    /// indices of the misses, in order. The caller embeds only the misses
    /// and stores them back via [`insert`].
    pub fn lookup_batch(
        &mut self,
        texts: &[&str],
        normalize: bool,
    ) -> (Vec<Option<Vec<f32>>>, Vec<usize>) {
        let mut found = Vec::with_capacity(texts.len());
        let mut miss_indices = Vec::new();
        for (i, text) in texts.iter().enumerate() {
            let slot = self.get(cache_key(text, normalize));
            if slot.is_none() {
                miss_indices.push(i);
            }
            found.push(slot);
        }
        (found, miss_indices)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_then_get_hits() {
        let mut c = EmbedCache::new(10);
        let k = cache_key("hello", true);
        c.insert(k, vec![1.0, 2.0]);
        assert_eq!(c.get(k), Some(vec![1.0, 2.0]));
        assert_eq!(c.hits, 1);
        assert_eq!(c.misses, 0);
    }

    #[test]
    fn miss_counts() {
        let mut c = EmbedCache::new(10);
        assert!(c.get(cache_key("nope", true)).is_none());
        assert_eq!(c.misses, 1);
    }

    #[test]
    fn normalize_flag_separates_keys() {
        assert_ne!(cache_key("same", true), cache_key("same", false));
    }

    #[test]
    fn eviction_is_fifo_and_respects_capacity() {
        let mut c = EmbedCache::new(2);
        c.insert(cache_key("a", true), vec![1.0]);
        c.insert(cache_key("b", true), vec![2.0]);
        c.insert(cache_key("c", true), vec![3.0]);
        assert_eq!(c.len(), 2);
        // "a" was oldest — evicted
        assert!(c.get(cache_key("a", true)).is_none());
        assert!(c.get(cache_key("b", true)).is_some());
        assert!(c.get(cache_key("c", true)).is_some());
    }

    #[test]
    fn reinsert_same_key_does_not_grow_order() {
        let mut c = EmbedCache::new(2);
        let k = cache_key("a", true);
        c.insert(k, vec![1.0]);
        c.insert(k, vec![1.5]);
        c.insert(cache_key("b", true), vec![2.0]);
        assert_eq!(c.len(), 2);
        // updated value preserved
        assert_eq!(c.get(k), Some(vec![1.5]));
    }

    #[test]
    fn lookup_batch_splits_hits_and_misses() {
        let mut c = EmbedCache::new(10);
        c.insert(cache_key("warm", true), vec![1.0]);
        let (found, misses) = c.lookup_batch(&["warm", "cold", "warm"], true);
        assert!(found[0].is_some());
        assert!(found[1].is_none());
        assert!(found[2].is_some());
        assert_eq!(misses, vec![1]);
    }
}
