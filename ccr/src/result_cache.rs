//! RC — Post-Pipeline Result Cache.
//!
//! Caches compressed bash outputs keyed by a hash of the raw text and command hint.
//! On a hit, the entire pipeline is bypassed and stored bytes are returned
//! byte-identically, guaranteeing stable content in Claude's conversation history
//! which maximises Anthropic prompt cache hits.
//!
//! Cache entries expire after 1 hour. Storage is per-session.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// TTL for cache entries in seconds.
const CACHE_TTL_SECS: u64 = 3_600;
/// Maximum number of entries per session cache file.
const MAX_ENTRIES: usize = 200;

#[derive(Serialize, Deserialize, Clone)]
pub struct ResultCacheEntry {
    /// Compressed output bytes (frozen — byte-identical on every hit).
    pub output: String,
    pub ts: u64,
    pub input_tokens: usize,
    pub output_tokens: usize,
}

#[derive(Serialize, Deserialize, Default)]
pub struct ResultCache {
    entries: HashMap<String, ResultCacheEntry>,
}

// ── Persistence ────────────────────────────────────────────────────────────────

fn storage_path(session_id: &str) -> Option<PathBuf> {
    Some(
        dirs::data_local_dir()?
            .join("ccr")
            .join("result_cache")
            .join(format!("{}.json", session_id)),
    )
}

impl ResultCache {
    pub fn load(session_id: &str) -> Self {
        storage_path(session_id)
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, session_id: &str) {
        let Some(path) = storage_path(session_id) else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let Ok(json) = serde_json::to_string(self) else { return };
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

// ── Key computation ────────────────────────────────────────────────────────────

impl ResultCache {
    /// Compute a 16-char hex key from raw text and command hint.
    /// Deliberately excludes query and session state so the first-compression
    /// result is frozen regardless of context changes on later turns.
    pub fn compute_key(raw_text: &str, command_hint: Option<&str>) -> String {
        crate::util::hash_str(&format!("{}\0{}", raw_text, command_hint.unwrap_or("")))
    }
}

// ── Lookup / insert / evict ───────────────────────────────────────────────────

impl ResultCache {
    /// Return a cached entry for `key`, or `None` on a miss.
    pub fn lookup(&self, key: &str) -> Option<&ResultCacheEntry> {
        self.entries.get(key)
    }

    /// Store a compressed result. Evicts the oldest entry when at capacity.
    pub fn insert(
        &mut self,
        key: String,
        output: String,
        input_tokens: usize,
        output_tokens: usize,
    ) {
        if self.entries.len() >= MAX_ENTRIES {
            // Evict the oldest entry by timestamp
            if let Some(oldest_key) = self
                .entries
                .iter()
                .min_by_key(|(_, v)| v.ts)
                .map(|(k, _)| k.clone())
            {
                self.entries.remove(&oldest_key);
            }
        }
        self.entries.insert(
            key,
            ResultCacheEntry {
                output,
                ts: now_secs(),
                input_tokens,
                output_tokens,
            },
        );
    }

    /// Remove entries older than the TTL.
    pub fn evict_old(&mut self) {
        let cutoff = now_secs().saturating_sub(CACHE_TTL_SECS);
        self.entries.retain(|_, v| v.ts >= cutoff);
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────────

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_key_is_deterministic() {
        let k1 = ResultCache::compute_key("hello", Some("git"));
        let k2 = ResultCache::compute_key("hello", Some("git"));
        assert_eq!(k1, k2);
    }

    #[test]
    fn compute_key_differs_by_text() {
        let k1 = ResultCache::compute_key("A", None);
        let k2 = ResultCache::compute_key("B", None);
        assert_ne!(k1, k2);
    }

    #[test]
    fn compute_key_differs_by_hint() {
        let k1 = ResultCache::compute_key("same text", Some("git"));
        let k2 = ResultCache::compute_key("same text", Some("cargo"));
        assert_ne!(k1, k2);
    }

    #[test]
    fn compute_key_none_hint_stable() {
        let k1 = ResultCache::compute_key("text", None);
        let k2 = ResultCache::compute_key("text", None);
        assert_eq!(k1, k2);
    }

    #[test]
    fn lookup_miss_then_hit() {
        let mut cache = ResultCache::default();
        let key = ResultCache::compute_key("output data", Some("cargo"));
        cache.insert(key.clone(), "compressed output".to_string(), 100, 20);
        let entry = cache.lookup(&key).unwrap();
        assert_eq!(entry.output, "compressed output");
        assert_eq!(entry.input_tokens, 100);
        assert_eq!(entry.output_tokens, 20);
    }

    #[test]
    fn evict_old_removes_stale() {
        let mut cache = ResultCache::default();
        let key = "testkey".to_string();
        cache.entries.insert(
            key.clone(),
            ResultCacheEntry {
                output: "old".to_string(),
                ts: now_secs().saturating_sub(CACHE_TTL_SECS + 1),
                input_tokens: 10,
                output_tokens: 5,
            },
        );
        cache.evict_old();
        assert!(cache.lookup(&key).is_none());
    }

    #[test]
    fn evict_old_keeps_fresh() {
        let mut cache = ResultCache::default();
        let key = ResultCache::compute_key("fresh data", None);
        cache.insert(key.clone(), "fresh output".to_string(), 50, 10);
        cache.evict_old();
        assert!(cache.lookup(&key).is_some());
    }

    #[test]
    fn max_entries_cap() {
        let mut cache = ResultCache::default();
        for i in 0..=MAX_ENTRIES {
            let key = ResultCache::compute_key(&format!("input {}", i), None);
            cache.insert(key, format!("output {}", i), 10, 5);
        }
        assert!(cache.entries.len() <= MAX_ENTRIES);
    }
}
