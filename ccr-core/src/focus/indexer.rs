//! Index builder — full + incremental build, git cochange extraction, embedding generation.
//!
//! Ported from atlas: builds a SQLite graph of files, their roles, co-change relationships,
//! and semantic embeddings to guide the agent toward relevant files.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use super::graph;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const MAX_FILE_SIZE_BYTES: u64 = 500 * 1024;
pub const MAX_COCHANGE_FILES_PER_COMMIT: usize = 50;
pub const LRU_KEEP_COUNT: usize = 5;
const EMBED_BATCH_SIZE: usize = 64;
const EMBED_CONTENT_CHARS: usize = 1000;

/// Directory names that are always skipped during full builds.
const SKIP_DIRS: &[&str] = &[
    ".git", ".idea", ".vscode", "node_modules", "target", ".gradle",
    "dist", "__pycache__", ".next", ".nuxt", "vendor", "third_party",
    ".cache", ".fastembed_cache", "build", "out", ".dart_tool", ".pub-cache",
    // Noise directories: test data, docs — not useful for code retrieval.
    // Note: "samples" and "example(s)" are intentionally omitted — Java packages
    // like org.springframework.samples.* and monorepo sub-package examples/ would
    // be incorrectly excluded. is_noise_path() handles these at query time instead.
    "testdata", "test-data", "fixtures", "fixture",
    "examples", "docs", "doc", "documentation", "docs_src",
    "e2e", "benchmarks", "website", "paradox", "i18n",
];

/// File extensions that are always skipped.
const SKIP_EXTENSIONS: &[&str] = &[
    "lock", "pb.go", "min.js", "min.css", "min.ts", "map", "svg", "png",
    "jpg", "jpeg", "gif", "ico", "woff", "woff2", "ttf", "eot", "pdf",
    "zip", "tar", "gz", "exe", "dll", "so", "dylib", "a", "o",
    "json", "xml", "iml",
    // Prose documentation — not useful for code retrieval
    "md", "mdx", "rst", "adoc", "asciidoc",
    // Java/Spring config files — not useful for code retrieval
    "properties", "jmx",
];

// ---------------------------------------------------------------------------
// Meta
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
pub struct Meta {
    pub head_hash: String,
    pub repo_root: String,
    pub schema_version: String,
    pub embedding_model: String,
    pub indexed_at: u64,
}

impl Meta {
    pub fn write(&self, index_dir: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        let path = index_dir.join("meta.json");
        fs::write(path, json)?;
        Ok(())
    }

    pub fn read(index_dir: &Path) -> Result<Self> {
        let json = fs::read_to_string(index_dir.join("meta.json"))?;
        Ok(serde_json::from_str(&json)?)
    }
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

pub fn should_skip_dir(name: &str) -> bool {
    SKIP_DIRS.contains(&name)
}

pub fn should_skip_file(path: &Path, size_bytes: u64) -> bool {
    if size_bytes > MAX_FILE_SIZE_BYTES {
        return true;
    }
    let name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return true,
    };
    if name.starts_with('.') {
        return true;
    }
    let lower = name.to_lowercase();
    for ext in SKIP_EXTENSIONS {
        if lower.ends_with(&format!(".{}", ext)) {
            return true;
        }
    }
    false
}

pub fn parse_cochange_log(output: &str) -> Vec<Vec<String>> {
    let mut commits: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();

    for line in output.lines() {
        let line = line.trim();
        if line == "ATLASCOMMIT" {
            if !current.is_empty() {
                if current.len() <= MAX_COCHANGE_FILES_PER_COMMIT {
                    commits.push(current.clone());
                }
                current.clear();
            }
        } else if !line.is_empty() {
            current.push(line.to_string());
        }
    }

    if !current.is_empty() && current.len() <= MAX_COCHANGE_FILES_PER_COMMIT {
        commits.push(current);
    }

    commits
}

pub fn build_cochange_pairs(commits: &[Vec<String>]) -> HashMap<(String, String), u32> {
    let mut counts: HashMap<(String, String), u32> = HashMap::new();
    for files in commits {
        let mut sorted = files.clone();
        sorted.sort();
        sorted.dedup();
        for i in 0..sorted.len() {
            for j in (i + 1)..sorted.len() {
                let key = (sorted[i].clone(), sorted[j].clone());
                *counts.entry(key).or_insert(0) += 1;
            }
        }
    }
    counts
}

pub fn list_index_dirs(index_parent: &Path) -> Vec<(PathBuf, u64)> {
    let entries = match fs::read_dir(index_parent) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut dirs: Vec<(PathBuf, u64)> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter(|e| {
            let name = e.file_name();
            let s = name.to_string_lossy();
            !s.contains("-build-") && !s.starts_with('.')
        })
        .filter_map(|e| {
            let dir = e.path();
            Meta::read(&dir).ok().map(|m| (dir, m.indexed_at))
        })
        .collect();

    dirs.sort_by(|a, b| b.1.cmp(&a.1)); // newest first
    dirs
}

pub fn evict_lru(index_parent: &Path, keep: usize) -> Result<()> {
    let dirs = list_index_dirs(index_parent);
    for (dir, _) in dirs.into_iter().skip(keep) {
        fs::remove_dir_all(&dir)
            .with_context(|| format!("failed to evict index dir {}", dir.display()))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Git helpers
// ---------------------------------------------------------------------------

pub fn current_head(repo_root: &Path) -> Result<String> {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_root)
        .output()?;
    if !out.status.success() {
        bail!("git rev-parse HEAD failed — no commits yet?");
    }
    Ok(String::from_utf8(out.stdout)?.trim().to_string())
}

pub fn git_cochange_log(repo_root: &Path, from: Option<&str>, to: &str) -> Result<String> {
    let range = match from {
        Some(f) => format!("{}..{}", f, to),
        None => to.to_string(),
    };
    let out = Command::new("git")
        .args([
            "log",
            &range,
            "--no-merges",
            "--name-only",
            "--format=tformat:ATLASCOMMIT",
        ])
        .current_dir(repo_root)
        .output()?;
    if !out.status.success() {
        bail!(
            "git log failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8(out.stdout)?)
}

// ---------------------------------------------------------------------------
// Build
// ---------------------------------------------------------------------------

pub fn run_index(repo_root: &Path, index_parent: &Path) -> Result<()> {
    fs::create_dir_all(index_parent)?;

    let head = current_head(repo_root)?;
    let head_dir = index_parent.join(&head);

    // Already valid — nothing to do
    if graph::is_valid(&head_dir.join("graph.sqlite")) {
        return Ok(());
    }

    // Find most recent prior valid index
    let prior = list_index_dirs(index_parent)
        .into_iter()
        .find(|(dir, _)| graph::is_valid(&dir.join("graph.sqlite")));

    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let temp_dir_name = format!("{}-build-{}-{}", head, std::process::id(), epoch);
    let temp_dir = index_parent.join(&temp_dir_name);
    fs::create_dir_all(&temp_dir)?;

    let db_path = temp_dir.join("graph.sqlite");
    let conn = graph::open_readwrite(&db_path)?;

    // Full build for now (incremental will be added later if needed)
    full_build(repo_root, &conn)?;

    // Extract cochange pairs from git history
    let prior_hash = prior.as_ref().and_then(|(dir, _)| {
        Meta::read(dir).ok().map(|m| m.head_hash)
    });

    let cochange_log = git_cochange_log(repo_root, prior_hash.as_deref(), &head)?;
    let commits = parse_cochange_log(&cochange_log);
    let pairs = build_cochange_pairs(&commits);
    upsert_cochanges(&conn, &pairs, epoch)?;

    let meta = Meta {
        head_hash: head.clone(),
        repo_root: repo_root.to_string_lossy().to_string(),
        schema_version: graph::SCHEMA_VERSION.to_string(),
        embedding_model: "AllMiniLML6V2".to_string(),
        indexed_at: epoch,
    };

    drop(conn); // close before rename

    // Remove stale target if it exists
    if head_dir.exists() {
        fs::remove_dir_all(&head_dir)
            .with_context(|| format!("failed to remove stale index dir {}", head_dir.display()))?;
    }

    // Atomic rename
    fs::rename(&temp_dir, &head_dir)
        .with_context(|| format!("failed to rename {} → {}", temp_dir.display(), head_dir.display()))?;

    meta.write(&head_dir)?;
    evict_lru(index_parent, LRU_KEEP_COUNT)?;

    Ok(())
}

/// Classify a file's role from its path.
/// Returns (role, confidence) where role is one of:
/// entry_point, test, persistence, state_manager, config, general.
pub fn classify_role(path: &str) -> (&'static str, f64) {
    let lower = path.to_lowercase();
    let basename = std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();

    // Entry points
    if matches!(
        basename.as_str(),
        "main.rs" | "main.py" | "main.go" | "main.ts" | "main.js"
            | "index.ts" | "index.js" | "index.tsx" | "index.jsx"
            | "app.py" | "app.ts" | "app.js" | "app.rs"
    ) || basename.starts_with("server.")
        || lower.contains("/cmd/")
        || lower.contains("/bin/")
    {
        return ("entry_point", 0.9);
    }

    // Tests
    if basename.ends_with("_test.rs")
        || basename.ends_with("_test.go")
        || basename.ends_with("_test.py")
        || basename.ends_with("_spec.ts")
        || basename.ends_with("_spec.js")
        || basename.ends_with(".test.ts")
        || basename.ends_with(".test.js")
        || basename.starts_with("test_")
        || lower.contains("/tests/")
        || lower.contains("/test/")
        || lower.contains("/spec/")
    {
        return ("test", 0.9);
    }

    // Persistence
    if lower.contains("db")
        || lower.contains("migration")
        || lower.contains("schema")
        || lower.contains("model")
        || lower.contains("repository")
    {
        return ("persistence", 0.8);
    }

    // State manager
    if lower.contains("store")
        || lower.contains("state")
        || lower.contains("reducer")
        || lower.contains("context")
    {
        return ("state_manager", 0.7);
    }

    // Config
    if basename.ends_with(".toml")
        || basename.ends_with(".yaml")
        || basename.ends_with(".yml")
        || basename.contains(".env")
        || basename.starts_with("config.")
    {
        return ("config", 0.7);
    }

    ("general", 0.5)
}

/// Convert a Vec<f32> embedding to a little-endian byte blob for SQLite storage.
fn embedding_to_blob(embedding: &[f32]) -> Vec<u8> {
    embedding
        .iter()
        .flat_map(|f| f.to_le_bytes())
        .collect()
}

/// A file collected during the walk, before embedding.
struct FileRecord {
    rel_path: String,
    role: &'static str,
    role_confidence: f64,
    embed_text: String,
    signatures: String,
}

fn full_build(repo_root: &Path, conn: &rusqlite::Connection) -> Result<()> {
    let walker = walkdir::WalkDir::new(repo_root)
        .into_iter()
        .filter_map(|e| e.ok());

    let epoch = now_secs();

    // Collect all files first, then batch-embed
    let mut files: Vec<FileRecord> = Vec::new();

    for entry in walker {
        let path = entry.path();

        // Skip directories (by name only — leaf component)
        if path.is_dir() {
            let name = path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if should_skip_dir(name) {
                continue;
            }
        } else if !path.is_file() {
            continue;
        }

        // Get relative path before component check so we don't match path components
        // from the repo_root itself (e.g. a "benchmarks/" parent dir in the host path).
        let rel = match path.strip_prefix(repo_root) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let rel_str = rel.to_string_lossy().to_string();

        // Skip files whose RELATIVE path contains a skipped directory component
        let skip = rel.components().any(|c| {
            let s = c.as_os_str().to_string_lossy();
            should_skip_dir(&s)
        });
        if skip {
            continue;
        }

        let size = fs::metadata(path).map(|m| m.len()).unwrap_or(u64::MAX);
        if should_skip_file(path, size) {
            continue;
        }

        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let (role, role_confidence) = classify_role(&rel_str);

        // Build embedding input: structural signatures (API surface) when available,
        // falling back to first N chars for unsupported types (data formats, shell, etc.)
        let sigs = crate::focus::symbols::apply_structural(&content, &rel_str);
        let embed_text = if sigs.trim().is_empty() {
            let content_prefix: String = content.chars().take(EMBED_CONTENT_CHARS).collect();
            format!("{}\n{}", rel_str, content_prefix)
        } else {
            format!("{}\n{}", rel_str, sigs)
        };

        files.push(FileRecord {
            rel_path: rel_str,
            role,
            role_confidence,
            embed_text,
            signatures: sigs,
        });
    }

    // Batch-embed files in chunks of EMBED_BATCH_SIZE
    for chunk in files.chunks(EMBED_BATCH_SIZE) {
        let texts: Vec<&str> = chunk.iter().map(|f| f.embed_text.as_str()).collect();

        let embeddings = crate::summarizer::embed_batch(&texts).unwrap_or_default();

        for (i, file) in chunk.iter().enumerate() {
            let blob = if i < embeddings.len() {
                embedding_to_blob(&embeddings[i])
            } else {
                Vec::new() // fallback: empty embedding if batch failed
            };

            conn.execute(
                "INSERT OR REPLACE INTO files
                 (path, role, role_confidence, commit_count, updated_at, symbols, signatures, embedding)
                 VALUES (?1, ?2, ?3, 0, ?4, '', ?5, ?6)",
                rusqlite::params![file.rel_path, file.role, file.role_confidence, epoch as i64, file.signatures, blob],
            )?;
        }
    }

    Ok(())
}

fn upsert_cochanges(
    conn: &rusqlite::Connection,
    pairs: &HashMap<(String, String), u32>,
    epoch: u64,
) -> Result<()> {
    for ((file_a, file_b), count) in pairs {
        conn.execute(
            "INSERT INTO cochanges (file_a, file_b, change_count, last_seen)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(file_a, file_b) DO UPDATE SET
                change_count = cochanges.change_count + ?3,
                last_seen = ?4",
            rusqlite::params![file_a, file_b, count, epoch as i64],
        )?;
    }
    Ok(())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_skip_dir() {
        assert!(should_skip_dir("node_modules"));
        assert!(should_skip_dir("target"));
        assert!(!should_skip_dir("src"));
    }

    #[test]
    fn test_should_skip_file() {
        assert!(should_skip_file(Path::new("file.json"), 100));
        assert!(should_skip_file(Path::new("file.min.js"), 100));
        assert!(!should_skip_file(Path::new("file.rs"), 100));
    }

    #[test]
    fn test_parse_cochange_log_empty() {
        let result = parse_cochange_log("");
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_cochange_log_single_commit() {
        let input = "src/a.rs\nsrc/b.rs\nATLASCOMMIT\n";
        let result = parse_cochange_log(input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], vec!["src/a.rs", "src/b.rs"]);
    }

    #[test]
    fn test_build_cochange_pairs() {
        let commits = vec![
            vec!["src/a.rs".to_string(), "src/b.rs".to_string()],
            vec!["src/a.rs".to_string(), "src/b.rs".to_string()],
        ];
        let pairs = build_cochange_pairs(&commits);
        assert_eq!(pairs[&("src/a.rs".to_string(), "src/b.rs".to_string())], 2);
    }
}
