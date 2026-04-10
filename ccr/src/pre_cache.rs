//! PC — Pre-run Cache.
//!
//! Computes a structural cache key BEFORE executing a command. If the key
//! matches a recent cached result, the command is skipped entirely and the
//! cached output is returned — saving both execution time and output tokens.
//!
//! Supported: git (status, diff, log, branch, stash),
//!            kubectl (get, describe),
//!            docker (ps, images),
//!            terraform (show, output).
//!
//! Cache entries expire according to per-command TTLs. Storage is per-session.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Default TTL for git entries (1 hour — git state is relatively stable).
const GIT_TTL_SECS: u64 = 3_600;
/// TTL for kubectl entries (60 s — cluster state changes faster).
const KUBECTL_TTL_SECS: u64 = 60;
/// TTL for docker entries (30 s — container state is volatile).
const DOCKER_TTL_SECS: u64 = 30;
/// TTL for terraform entries (5 min — state file rarely changes mid-session).
const TERRAFORM_TTL_SECS: u64 = 300;

#[derive(Clone)]
pub struct PreCacheKey {
    /// 16-char hex key; stable when the relevant state has not changed.
    pub key: String,
    /// The command string used as the HashMap key (e.g. "git status").
    pub cmd: String,
    /// TTL for this entry in seconds.
    pub ttl_secs: u64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct PreCacheEntry {
    pub key: String,
    pub output: String,
    pub ts: u64,
    /// Token count of `output` at time of caching.
    pub tokens: usize,
}

#[derive(Serialize, Deserialize, Default)]
pub struct PreCache {
    entries: HashMap<String, PreCacheEntry>,
}

// ── Persistence ────────────────────────────────────────────────────────────────

fn cache_path(session_id: &str) -> Option<PathBuf> {
    Some(
        dirs::data_local_dir()?
            .join("panda")
            .join("pre_cache")
            .join(format!("{}.json", session_id)),
    )
}

impl PreCache {
    pub fn load(session_id: &str) -> Self {
        cache_path(session_id)
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, session_id: &str) {
        let Some(path) = cache_path(session_id) else { return };
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

impl PreCache {
    /// Compute a structural cache key for `args` before executing the command.
    /// Returns `None` if the command is not cacheable or state cannot be determined.
    pub fn compute_key(args: &[String]) -> Option<PreCacheKey> {
        match args.first().map(|s| s.as_str()) {
            Some("git") => git_cache_key(args),
            Some("kubectl") => kubectl_cache_key(args),
            Some("docker") => docker_cache_key(args),
            Some("terraform") | Some("tofu") => terraform_cache_key(args),
            _ => None,
        }
    }
}

fn git_cache_key(args: &[String]) -> Option<PreCacheKey> {
    let subcmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
    match subcmd {
        "status" | "diff" | "log" | "branch" | "stash" => {}
        _ => return None,
    }
    let cmd_str = args.iter().take(2).cloned().collect::<Vec<_>>().join(" ");

    // HEAD hash
    let head = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !head.status.success() {
        return None;
    }
    let head_hash = String::from_utf8_lossy(&head.stdout).trim().to_string();

    // Staged changes summary
    let staged = std::process::Command::new("git")
        .args(["diff-index", "--cached", "--stat", "HEAD"])
        .output()
        .ok()?;

    // Unstaged changes summary
    let unstaged = std::process::Command::new("git")
        .args(["diff-files", "--stat"])
        .output()
        .ok()?;

    let combined = format!(
        "{}{}{}",
        head_hash,
        String::from_utf8_lossy(&staged.stdout),
        String::from_utf8_lossy(&unstaged.stdout)
    );
    let key = crate::util::hash_str(&combined);

    Some(PreCacheKey { key, cmd: cmd_str, ttl_secs: GIT_TTL_SECS })
}

fn kubectl_cache_key(args: &[String]) -> Option<PreCacheKey> {
    let subcmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
    match subcmd {
        "get" | "describe" => {}
        _ => return None,
    }
    let cmd_str = args.iter().take(3).cloned().collect::<Vec<_>>().join(" ");

    // Extract namespace from -n / --namespace flags (or use "default")
    let ns = extract_flag_value(args, "-n")
        .or_else(|| extract_flag_value(args, "--namespace"))
        .unwrap_or_else(|| "default".to_string());

    // Quick state probe — 3s timeout so an unreachable cluster never hangs the user
    let probe = std::process::Command::new("kubectl")
        .args(["get", "all", "-n", &ns, "--no-headers", "-o", "name", "--request-timeout=3s"])
        .output()
        .ok()?;
    if !probe.status.success() {
        return None;
    }

    let key = crate::util::hash_str(&String::from_utf8_lossy(&probe.stdout));
    Some(PreCacheKey { key, cmd: cmd_str, ttl_secs: KUBECTL_TTL_SECS })
}

fn docker_cache_key(args: &[String]) -> Option<PreCacheKey> {
    let subcmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
    match subcmd {
        "ps" | "images" => {}
        _ => return None,
    }
    let cmd_str = args.iter().take(2).cloned().collect::<Vec<_>>().join(" ");

    // Quick probes: container IDs + image IDs (fast, stable)
    let ps_ids = std::process::Command::new("docker")
        .args(["ps", "-q"])
        .output()
        .ok()?;
    let img_ids = std::process::Command::new("docker")
        .args(["images", "-q"])
        .output()
        .ok()?;

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&ps_ids.stdout),
        String::from_utf8_lossy(&img_ids.stdout)
    );
    let key = crate::util::hash_str(&combined);
    Some(PreCacheKey { key, cmd: cmd_str, ttl_secs: DOCKER_TTL_SECS })
}

fn terraform_cache_key(args: &[String]) -> Option<PreCacheKey> {
    let subcmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
    match subcmd {
        "show" | "output" => {}
        _ => return None,
    }
    let cmd_str = args.iter().take(2).cloned().collect::<Vec<_>>().join(" ");

    // Probe: terraform show -json for a stable hash of state
    let probe = std::process::Command::new("terraform")
        .args(["show", "-json"])
        .output()
        .ok()?;
    if !probe.status.success() || probe.stdout.is_empty() {
        return None;
    }

    // Use SHA-256 of the JSON output for stability
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(&probe.stdout);
    let key = hex::encode(&digest[..8]); // 16 hex chars

    Some(PreCacheKey { key, cmd: cmd_str, ttl_secs: TERRAFORM_TTL_SECS })
}

// ── Lookup / insert / evict ───────────────────────────────────────────────────

impl PreCache {
    /// Look up a cache entry. Returns `Some` only when the key hash matches exactly
    /// and the entry has not exceeded its TTL.
    pub fn lookup(&self, key: &PreCacheKey) -> Option<&PreCacheEntry> {
        let entry = self.entries.get(&key.cmd)?;
        if entry.key != key.key {
            return None;
        }
        // Check per-command TTL
        if now_secs().saturating_sub(entry.ts) > key.ttl_secs {
            return None;
        }
        Some(entry)
    }

    /// Store or update a cache entry.
    pub fn insert(&mut self, key: PreCacheKey, output: &str, tokens: usize) {
        self.entries.insert(
            key.cmd.clone(),
            PreCacheEntry {
                key: key.key,
                output: output.to_string(),
                ts: now_secs(),
                tokens,
            },
        );
    }

    /// Remove entries older than the longest possible TTL (1 hour).
    pub fn evict_old(&mut self) {
        // Use the maximum TTL across all commands as the eviction boundary.
        // This is conservative — per-command TTL is also checked in lookup().
        let max_ttl = GIT_TTL_SECS; // 1 hour is the largest TTL
        let cutoff = now_secs().saturating_sub(max_ttl);
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

/// Extract the value of a flag like `-n <value>` or `--namespace <value>` from args.
fn extract_flag_value<'a>(args: &'a [String], flag: &str) -> Option<String> {
    let pos = args.iter().position(|a| a == flag)?;
    args.get(pos + 1).cloned()
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(key: &str, cmd: &str, ttl: u64) -> PreCacheKey {
        PreCacheKey { key: key.to_string(), cmd: cmd.to_string(), ttl_secs: ttl }
    }

    #[test]
    fn compute_key_returns_none_for_unknown_command() {
        let result = PreCache::compute_key(&["python3".into(), "main.py".into()]);
        assert!(result.is_none());
    }

    #[test]
    fn compute_key_returns_none_for_git_push() {
        let result = PreCache::compute_key(&["git".into(), "push".into()]);
        assert!(result.is_none());
    }

    #[test]
    fn compute_key_returns_none_for_git_commit() {
        let result = PreCache::compute_key(&["git".into(), "commit".into()]);
        assert!(result.is_none());
    }

    #[test]
    fn compute_key_returns_none_for_docker_build() {
        let result = PreCache::compute_key(&["docker".into(), "build".into(), ".".into()]);
        assert!(result.is_none());
    }

    #[test]
    fn compute_key_returns_none_for_terraform_plan() {
        let result = PreCache::compute_key(&["terraform".into(), "plan".into()]);
        assert!(result.is_none());
    }

    #[test]
    fn lookup_returns_none_on_key_mismatch() {
        let mut cache = PreCache::default();
        cache.insert(make_key("aabbccdd11223344", "git status", GIT_TTL_SECS), "output", 100);
        let result = cache.lookup(&make_key("eeff001122334455", "git status", GIT_TTL_SECS));
        assert!(result.is_none());
    }

    #[test]
    fn lookup_returns_entry_on_key_match() {
        let mut cache = PreCache::default();
        cache.insert(make_key("aabbccdd11223344", "git status", GIT_TTL_SECS), "some output", 42);
        let result = cache.lookup(&make_key("aabbccdd11223344", "git status", GIT_TTL_SECS));
        assert!(result.is_some());
        assert_eq!(result.unwrap().output, "some output");
    }

    #[test]
    fn lookup_returns_none_for_expired_ttl() {
        let mut cache = PreCache::default();
        // Insert with a fake old timestamp
        cache.entries.insert(
            "docker ps".to_string(),
            PreCacheEntry {
                key: "abc".to_string(),
                output: "old".to_string(),
                ts: now_secs().saturating_sub(DOCKER_TTL_SECS + 10),
                tokens: 10,
            },
        );
        // Lookup with DOCKER_TTL_SECS — should be expired
        let result = cache.lookup(&make_key("abc", "docker ps", DOCKER_TTL_SECS));
        assert!(result.is_none(), "entry past TTL should not be returned");
    }

    #[test]
    fn lookup_respects_per_command_ttl() {
        let mut cache = PreCache::default();
        // Insert an entry 35 seconds old
        let age = 35u64;
        cache.entries.insert(
            "docker ps".to_string(),
            PreCacheEntry {
                key: "abc".to_string(),
                output: "containers".to_string(),
                ts: now_secs().saturating_sub(age),
                tokens: 10,
            },
        );
        // Docker TTL is 30s → expired
        let result = cache.lookup(&make_key("abc", "docker ps", DOCKER_TTL_SECS));
        assert!(result.is_none(), "docker entry > 30s should be expired");
        // Git TTL is 3600s → still valid (if same cache entry)
        cache.entries.insert(
            "git status".to_string(),
            PreCacheEntry {
                key: "abc".to_string(),
                output: "clean".to_string(),
                ts: now_secs().saturating_sub(age),
                tokens: 5,
            },
        );
        let result = cache.lookup(&make_key("abc", "git status", GIT_TTL_SECS));
        assert!(result.is_some(), "git entry < 3600s should still be valid");
    }

    #[test]
    fn evict_old_removes_stale_entries() {
        let mut cache = PreCache::default();
        cache.entries.insert(
            "git status".to_string(),
            PreCacheEntry {
                key: "abc".to_string(),
                output: "old".to_string(),
                ts: now_secs().saturating_sub(GIT_TTL_SECS + 100),
                tokens: 10,
            },
        );
        cache.evict_old();
        assert!(cache.entries.get("git status").is_none());
    }

    #[test]
    fn evict_old_keeps_fresh_entries() {
        let mut cache = PreCache::default();
        cache.insert(make_key("abc123", "git status", GIT_TTL_SECS), "fresh", 10);
        cache.evict_old();
        assert!(cache.entries.get("git status").is_some());
    }

    #[test]
    fn insert_overwrites_existing_cmd() {
        let mut cache = PreCache::default();
        cache.insert(make_key("key1", "git status", GIT_TTL_SECS), "old output", 10);
        cache.insert(make_key("key2", "git status", GIT_TTL_SECS), "new output", 20);
        let result = cache.lookup(&make_key("key2", "git status", GIT_TTL_SECS));
        assert!(result.is_some());
        assert_eq!(result.unwrap().output, "new output");
    }

    #[test]
    fn extract_flag_value_finds_namespace() {
        let args: Vec<String> = vec!["kubectl".into(), "get".into(), "-n".into(), "kube-system".into(), "pods".into()];
        assert_eq!(extract_flag_value(&args, "-n"), Some("kube-system".to_string()));
        assert_eq!(extract_flag_value(&args, "--namespace"), None);
    }
}
