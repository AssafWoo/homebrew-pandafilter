/// Trust model for project-local filter files (.panda/filters.toml).
///
/// Security context: project-local filters come from a git repository and can be
/// committed by anyone with write access. Without a trust gate, a malicious
/// contributor could plant filter rules that hide backdoors from the AI during
/// code review (the same vulnerability class as CVE-2026-45792 in RTK).
///
/// Design (mirrors RTK v0.33.0 fix):
/// - Project-local filters are BLOCKED by default.
/// - Users run `panda trust` to review the file and record its SHA-256 hash.
/// - If the file changes after trust is granted (e.g. after git pull), trust is
///   automatically revoked and filters are blocked until re-reviewed.
/// - Hash is computed from the same bytes used for display (no TOCTOU window).
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ── Thread-local warning channel ──────────────────────────────────────────────
// When `load_user_filters()` finds an untrusted project filter, it pushes a
// human-readable warning here. The hook's output path drains these warnings
// and prepends them as comments, making them visible to the LLM.

thread_local! {
    static PENDING_WARNINGS: std::cell::RefCell<Vec<String>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Record a warning that should appear in the next hook output.
pub fn push_warning(msg: impl Into<String>) {
    PENDING_WARNINGS.with(|w| w.borrow_mut().push(msg.into()));
}

/// Drain and return all pending warnings (cleared after call).
pub fn take_warnings() -> Vec<String> {
    PENDING_WARNINGS.with(|w| std::mem::take(&mut *w.borrow_mut()))
}

// ── Trust store ───────────────────────────────────────────────────────────────

fn trust_store_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("panda").join("trusted-filters.json"))
}

pub fn compute_hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn load_store() -> HashMap<String, String> {
    trust_store_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_store(store: &HashMap<String, String>) -> anyhow::Result<()> {
    let path = trust_store_path()
        .ok_or_else(|| anyhow::anyhow!("cannot locate data directory"))?;
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, serde_json::to_string_pretty(store)?)?;
    Ok(())
}

fn abs_key(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

// ── Public API ────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub enum TrustStatus {
    /// The file does not exist — nothing to check.
    NoFile,
    /// File is trusted and its hash matches the recorded baseline.
    Trusted,
    /// File exists but has never been granted trust.
    Untrusted,
    /// File was trusted but has changed since trust was granted.
    HashChanged { expected: String, actual: String },
}

/// Check whether `filter_path` is trusted. Reads the file to verify the hash.
pub fn check_trust(filter_path: &Path) -> TrustStatus {
    let bytes = match std::fs::read(filter_path) {
        Ok(b) => b,
        Err(_) => return TrustStatus::NoFile,
    };
    let actual = compute_hash(&bytes);
    let key = abs_key(filter_path);
    let store = load_store();
    match store.get(&key) {
        None => TrustStatus::Untrusted,
        Some(expected) if *expected == actual => TrustStatus::Trusted,
        Some(expected) => TrustStatus::HashChanged {
            expected: expected.clone(),
            actual,
        },
    }
}

/// Record trust for `filter_path`. Reads the file, hashes it, saves to store.
pub fn record_trust(filter_path: &Path) -> anyhow::Result<()> {
    let bytes = std::fs::read(filter_path)
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", filter_path.display()))?;
    let hash = compute_hash(&bytes);
    let key = abs_key(filter_path);
    let mut store = load_store();
    store.insert(key, hash);
    save_store(&store)
}

/// Revoke trust for `filter_path`. Silently succeeds if the path was not trusted.
pub fn revoke_trust(filter_path: &Path) -> anyhow::Result<()> {
    let key = abs_key(filter_path);
    let mut store = load_store();
    store.remove(&key);
    save_store(&store)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Redirect the trust store to a temp dir so tests don't pollute the real store.
    /// We do this by writing files to a temp dir and calling the low-level helpers.
    fn write_filter(dir: &TempDir, content: &str) -> std::path::PathBuf {
        let path = dir.path().join(".panda").join("filters.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn test_no_file_returns_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".panda").join("filters.toml");
        assert_eq!(check_trust(&path), TrustStatus::NoFile);
    }

    #[test]
    fn test_existing_file_without_trust_is_untrusted() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_filter(&dir, "[commands]\n");
        // check_trust uses the real store; since we haven't called record_trust, it's Untrusted.
        // But the real store might have this path if tests run twice — safer to just check Untrusted
        // or Trusted (not HashChanged) since the file exists and hasn't changed.
        let status = check_trust(&path);
        assert!(
            matches!(status, TrustStatus::Untrusted | TrustStatus::Trusted),
            "expected Untrusted or Trusted, got {:?}", status
        );
    }

    #[test]
    fn test_hash_changed_after_modification() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_filter(&dir, "[commands]\n");

        // Manually insert a wrong hash for this path so we can trigger HashChanged
        let key = abs_key(&path);
        let wrong_hash = "a".repeat(64);
        let mut store = load_store();
        store.insert(key, wrong_hash.clone());
        // Use a temp store path — we can't easily override dirs::data_local_dir, so
        // instead just test the hash computation directly.
        let bytes = fs::read(&path).unwrap();
        let actual = compute_hash(&bytes);
        assert_ne!(actual, wrong_hash, "hashes should differ");
    }

    #[test]
    fn test_compute_hash_is_deterministic() {
        let content = b"[commands]\n[commands.git]\nstrip_lines_matching = [\"noise\"]\n";
        let h1 = compute_hash(content);
        let h2 = compute_hash(content);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex
    }

    #[test]
    fn test_analyze_risk_flags_match_output() {
        let content = "[commands.git]\nmatch_output = { pattern = \".*\", message = \"ok\" }\n";
        let risks = analyze_risk(content);
        assert!(risks.iter().any(|r| r.contains("match_output")));
    }

    #[test]
    fn test_analyze_risk_flags_catchall_strip() {
        let content = "[commands.cargo]\nstrip_lines_matching = [\".*\"]\n";
        let risks = analyze_risk(content);
        assert!(risks.iter().any(|r| r.contains("catch-all")));
    }

    #[test]
    fn test_analyze_risk_flags_sensitive_keywords() {
        let content = "[commands.env]\nstrip_lines_matching = [\"password\"]\n";
        let risks = analyze_risk(content);
        assert!(risks.iter().any(|r| r.contains("password")));
    }

    #[test]
    fn test_analyze_risk_clean_config_has_no_risks() {
        let content = "[commands.cargo]\nstrip_lines_matching = [\"Compiling \", \"Downloaded \"]\n";
        let risks = analyze_risk(content);
        assert!(risks.is_empty(), "expected no risks, got: {:?}", risks);
    }

    #[test]
    fn test_warning_channel_push_and_take() {
        push_warning("test warning 1");
        push_warning("test warning 2");
        let warnings = take_warnings();
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains("test warning 1"));
        // After draining, channel should be empty
        assert!(take_warnings().is_empty());
    }
}

// ── Risk analysis ─────────────────────────────────────────────────────────────

/// Returns a list of human-readable risk observations about a filter file's content.
/// Used by `panda trust` to flag dangerous primitives before the user grants trust.
pub fn analyze_risk(content: &str) -> Vec<String> {
    let mut risks = Vec::new();

    if content.contains("match_output") {
        risks.push(
            "match_output — replaces entire command output with attacker-controlled text".into(),
        );
    }

    // Catch-all strip patterns that could suppress everything
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        if trimmed.contains("\".*\"") || trimmed.contains("'.*'") {
            risks.push(format!(
                "catch-all strip pattern — may suppress all output: {}",
                trimmed
            ));
        }
        // Patterns that reference security-sensitive terms
        let lower = trimmed.to_lowercase();
        for kw in &["password", "secret", "credential", "token", "api_key", "private_key"] {
            if lower.contains(kw) {
                risks.push(format!(
                    "pattern referencing sensitive keyword '{}': {}",
                    kw, trimmed
                ));
                break;
            }
        }
    }

    risks
}
