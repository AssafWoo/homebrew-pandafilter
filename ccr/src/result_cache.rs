//! RC — Post-Pipeline Result Cache.
//!
//! Two complementary tiers:
//!
//! 1. **Raw cache** (per-session + global cross-session, 24 h):
//!    Keyed by hash(raw_text + hint). On hit, returns byte-identical compressed
//!    output — guarantees Anthropic prompt-cache stability AND skips the full
//!    pipeline on repeated inputs across sessions.
//!
//! 2. **Normalized redirect cache** (per-session, 1 h):
//!    Keyed by hash(strip_temporal_noise(text) + hint). On hit, emits a
//!    ~15-token "output unchanged" marker instead of the full compressed output,
//!    saving tokens when the same command re-runs with only clock/UUID noise
//!    differing. Checked before the BERT polling suppressor; fires O(1).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// TTL for per-session raw entries and normalized-redirect entries (1 h).
const CACHE_TTL_SECS: u64 = 3_600;
/// TTL for cross-session global raw entries (24 h).
const GLOBAL_TTL_SECS: u64 = 86_400;
/// Maximum entries per cache file.
const MAX_ENTRIES: usize = 200;

#[derive(Serialize, Deserialize, Clone)]
pub struct ResultCacheEntry {
    /// Compressed output bytes (frozen — byte-identical on every hit).
    pub output: String,
    pub ts: u64,
    pub input_tokens: usize,
    pub output_tokens: usize,
}

/// Tracks a previous compressed result for redirect-on-repeat purposes.
/// Stored per-session; turn_num lets us emit "same as turn N" markers.
#[derive(Serialize, Deserialize, Clone)]
pub struct NormCacheEntry {
    pub turn_num: usize,
    pub output_tokens: usize,
    pub ts: u64,
}

#[derive(Serialize, Deserialize, Default)]
pub struct ResultCache {
    entries: HashMap<String, ResultCacheEntry>,
    #[serde(default)]
    norm_entries: HashMap<String, NormCacheEntry>,
}

// ── Persistence ────────────────────────────────────────────────────────────────

fn storage_path(session_id: &str) -> Option<PathBuf> {
    Some(
        dirs::data_local_dir()?
            .join("panda")
            .join("result_cache")
            .join(format!("{}.json", session_id)),
    )
}

fn global_storage_path() -> Option<PathBuf> {
    Some(
        dirs::data_local_dir()?
            .join("panda")
            .join("result_cache")
            .join("global.json"),
    )
}

fn save_to(cache: &ResultCache, path: &PathBuf) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(json) = serde_json::to_string(cache) else { return };
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
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
        save_to(self, &path);
    }

    /// Load the cross-session global raw cache. `norm_entries` are left empty —
    /// redirect turn numbers are session-scoped and meaningless across sessions.
    pub fn load_global() -> Self {
        global_storage_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<ResultCache>(&s).ok())
            .map(|mut c| { c.norm_entries.clear(); c })
            .unwrap_or_default()
    }

    /// Persist only the raw `entries` to the global file (norm entries are
    /// session-scoped and must not leak cross-session).
    pub fn save_global(&self) {
        let Some(path) = global_storage_path() else { return };
        let slim = ResultCache {
            entries: self.entries.clone(),
            norm_entries: HashMap::new(),
        };
        save_to(&slim, &path);
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

    /// Compute a redirect key from temporally-normalized text.
    /// `normalized_text` should be the output of `strip_temporal_noise` (defined
    /// in hook.rs) — timestamps, UUIDs, durations, git SHAs stripped.
    /// The "norm\0" prefix ensures no collision with raw keys.
    pub fn compute_normalized_key(normalized_text: &str, command_hint: Option<&str>) -> String {
        crate::util::hash_str(&format!(
            "norm\0{}\0{}",
            normalized_text,
            command_hint.unwrap_or("")
        ))
    }
}

// ── Lookup / insert / evict ───────────────────────────────────────────────────

impl ResultCache {
    /// Return a cached raw entry for `key`, or `None` on a miss.
    pub fn lookup(&self, key: &str) -> Option<&ResultCacheEntry> {
        self.entries.get(key)
    }

    /// Return a normalized redirect entry, or `None` on a miss.
    pub fn lookup_normalized(&self, key: &str) -> Option<&NormCacheEntry> {
        self.norm_entries.get(key)
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

    /// Record that normalized key `key` was seen at turn `turn_num` and
    /// produced `output_tokens` tokens. Evicts oldest when at capacity.
    pub fn insert_normalized(&mut self, key: String, turn_num: usize, output_tokens: usize) {
        if self.norm_entries.len() >= MAX_ENTRIES {
            if let Some(oldest_key) = self
                .norm_entries
                .iter()
                .min_by_key(|(_, v)| v.ts)
                .map(|(k, _)| k.clone())
            {
                self.norm_entries.remove(&oldest_key);
            }
        }
        self.norm_entries.insert(
            key,
            NormCacheEntry {
                turn_num,
                output_tokens,
                ts: now_secs(),
            },
        );
    }

    /// Remove entries older than their respective TTLs.
    pub fn evict_old(&mut self) {
        let raw_cutoff = now_secs().saturating_sub(CACHE_TTL_SECS);
        self.entries.retain(|_, v| v.ts >= raw_cutoff);
        let norm_cutoff = now_secs().saturating_sub(CACHE_TTL_SECS);
        self.norm_entries.retain(|_, v| v.ts >= norm_cutoff);
    }

    /// Remove global raw entries older than the 24 h TTL.
    pub fn evict_global_old(&mut self) {
        let cutoff = now_secs().saturating_sub(GLOBAL_TTL_SECS);
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

    #[test]
    fn normalized_key_differs_from_raw_key() {
        let raw = ResultCache::compute_key("same text", Some("cargo test"));
        let norm = ResultCache::compute_normalized_key("same text", Some("cargo test"));
        assert_ne!(raw, norm, "norm key must not collide with raw key");
    }

    #[test]
    fn normalized_key_is_deterministic() {
        let k1 = ResultCache::compute_normalized_key("output with 2024-01-01 timestamp", Some("git"));
        let k2 = ResultCache::compute_normalized_key("output with 2024-01-01 timestamp", Some("git"));
        assert_eq!(k1, k2);
    }

    #[test]
    fn norm_lookup_miss_then_hit() {
        let mut cache = ResultCache::default();
        let key = ResultCache::compute_normalized_key("test output", Some("cargo test"));
        assert!(cache.lookup_normalized(&key).is_none());
        cache.insert_normalized(key.clone(), 3, 120);
        let entry = cache.lookup_normalized(&key).unwrap();
        assert_eq!(entry.turn_num, 3);
        assert_eq!(entry.output_tokens, 120);
    }

    #[test]
    fn norm_entries_excluded_from_global_save() {
        let mut cache = ResultCache::default();
        cache.insert_normalized("norm_key".to_string(), 1, 50);
        let global = ResultCache {
            entries: cache.entries.clone(),
            norm_entries: std::collections::HashMap::new(),
        };
        assert!(global.lookup_normalized("norm_key").is_none());
    }

    #[test]
    fn norm_entries_evicted_by_evict_old() {
        let mut cache = ResultCache::default();
        let key = "old_norm".to_string();
        cache.norm_entries.insert(
            key.clone(),
            NormCacheEntry {
                turn_num: 1,
                output_tokens: 10,
                ts: now_secs().saturating_sub(CACHE_TTL_SECS + 1),
            },
        );
        cache.evict_old();
        assert!(cache.lookup_normalized(&key).is_none());
    }
}
