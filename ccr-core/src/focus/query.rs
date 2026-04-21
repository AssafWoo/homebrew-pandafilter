//! Query module — rank files by relevance using embeddings and cochanges.

use anyhow::Result;
use rusqlite::Connection;
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct RankedFile {
    pub path: String,
    pub role: String,
    pub confidence: f64,
    pub cochange_count: i64,
    pub relevance_score: f64,
}

/// Query the focus graph for relevant files given a prompt embedding.
///
/// Returns files ranked by a combination of:
/// 1. Semantic similarity (embedding distance)  — weight 0.5
/// 2. Co-change frequency (log-normalized)      — weight 0.2
/// 3. Read history boost (if provided)          — weight 0.3
/// 4. Role classification multiplier
pub fn query(
    conn: &Connection,
    prompt_embedding: &[f32],
    top_k: usize,
) -> Result<Vec<RankedFile>> {
    query_with_read_boosts(conn, prompt_embedding, top_k, None)
}

/// Like `query` but accepts optional read-history boosts (file_path → normalized frequency).
pub fn query_with_read_boosts(
    conn: &Connection,
    prompt_embedding: &[f32],
    top_k: usize,
    read_boosts: Option<&std::collections::HashMap<String, f64>>,
) -> Result<Vec<RankedFile>> {
    // Pass 1: collect raw scores
    let mut stmt = conn.prepare(
        "SELECT path, role, role_confidence, embedding, commit_count FROM files"
    )?;

    struct RawCandidate {
        path: String,
        role: String,
        confidence: f64,
        similarity: f64,
        raw_cochange: i64,
    }

    let mut raw_candidates: Vec<RawCandidate> = Vec::new();
    let rows = stmt.query_map([], |row| {
        let path: String = row.get(0)?;
        let role: String = row.get(1)?;
        let confidence: f64 = row.get(2)?;
        let blob: Vec<u8> = row.get(3)?;
        Ok((path, role, confidence, blob))
    })?;

    for row_result in rows {
        let (path, role, confidence, blob) = row_result?;
        // Skip files in directories that are noise for retrieval queries
        if is_noise_path(&path) {
            continue;
        }
        let file_embedding = blob_to_embedding(&blob);
        let similarity = cosine_similarity(prompt_embedding, &file_embedding);
        let raw_cochange = get_cochange_score(conn, &path)?;
        raw_candidates.push(RawCandidate {
            path,
            role,
            confidence,
            similarity,
            raw_cochange,
        });
    }

    // Pass 2: log-normalize co-change scores to [0, 1]
    let max_cochange = raw_candidates
        .iter()
        .map(|c| c.raw_cochange)
        .max()
        .unwrap_or(0);
    let log_max = (1.0 + max_cochange as f64).ln();

    let has_read_boosts = read_boosts.map_or(false, |rb| !rb.is_empty());

    // Weights: if read boosts available, use 0.5/0.2/0.3; otherwise 0.7/0.3/0
    let (w_sim, w_cochange, w_read) = if has_read_boosts {
        (0.5, 0.2, 0.3)
    } else {
        (0.7, 0.3, 0.0)
    };

    let mut candidates: Vec<(String, String, f64, i64, f64)> = raw_candidates
        .into_iter()
        .map(|c| {
            let norm_cochange = if log_max > 0.0 {
                (1.0 + c.raw_cochange as f64).ln() / log_max
            } else {
                0.0
            };

            let read_boost = if w_read > 0.0 {
                read_boosts
                    .and_then(|rb| rb.get(&c.path))
                    .copied()
                    .unwrap_or(0.0)
            } else {
                0.0
            };

            let relevance = c.similarity * w_sim + norm_cochange * w_cochange + read_boost * w_read;

            let role_boost = match c.role.as_str() {
                "entry_point" => 1.5,
                "persistence" => 1.2,
                "state_manager" => 1.1,
                _ => 1.0,
            };

            (c.path, c.role, c.confidence, c.raw_cochange, relevance * role_boost)
        })
        .collect();

    candidates.sort_by(|a, b| b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal));

    Ok(candidates
        .into_iter()
        .take(top_k)
        .map(|(path, role, confidence, cochange_count, relevance_score)| {
            RankedFile {
                path,
                role,
                confidence,
                cochange_count,
                relevance_score,
            }
        })
        .collect())
}

/// Get cochange score for a file (sum of all co-occurrence counts)
fn get_cochange_score(conn: &Connection, file_path: &str) -> Result<i64> {
    let score: i64 = conn.query_row(
        "SELECT COALESCE(SUM(change_count), 0) FROM cochanges
         WHERE file_a = ?1 OR file_b = ?1",
        [file_path],
        |row| row.get(0),
    )?;
    Ok(score)
}

/// Convert 4-byte blob to embedding vector
fn blob_to_embedding(blob: &[u8]) -> Vec<f32> {
    blob.chunks(4)
        .map(|chunk| {
            if chunk.len() == 4 {
                f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
            } else {
                0.0
            }
        })
        .collect()
}

/// Hybrid ranking: BERT (0.65) + lexical token matching on stored signatures (0.35).
///
/// Oversamples BERT candidates 3× then re-ranks using SigMap-inspired lexical scoring
/// on the `signatures` column (function/struct/type names extracted at index time).
pub fn query_hybrid(
    conn: &Connection,
    query_text: &str,
    embedding: &[f32],
    top_k: usize,
) -> Result<Vec<RankedFile>> {
    // Oversample BERT candidates 10× to ensure lexically-strong files that BERT ranks
    // in positions 6-50 still get a chance to surface after hybrid reranking.
    // Larger repos with many test/example files (e.g. fastapi) need more headroom.
    let candidates = query_with_read_boosts(conn, embedding, top_k * 10, None)?;

    let query_tokens = tokenize_query(query_text);
    let mut scored: Vec<RankedFile> = candidates
        .into_iter()
        .map(|mut f| {
            let sigs = get_signatures(conn, &f.path).unwrap_or_default();
            let lex = score_lexical(&sigs, &f.path, &query_tokens);
            f.relevance_score = 0.40 * f.relevance_score + 0.60 * lex;
            f
        })
        .collect();

    scored.sort_by(|a, b| {
        b.relevance_score
            .partial_cmp(&a.relevance_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(top_k);
    Ok(scored)
}

/// Returns true for files that are noise for code retrieval queries:
/// prose documentation, test data, and shallow example/sample directories.
fn is_noise_path(path: &str) -> bool {
    let p = path.replace('\\', "/").to_lowercase();

    // Prose file extensions — documentation, not code
    let prose_exts = [".md", ".mdx", ".rst", ".adoc", ".asciidoc", ".txt"];
    if prose_exts.iter().any(|e| p.ends_with(e)) {
        return true;
    }

    // Always-noisy directory segments at any depth
    let always_noisy = [
        "/testdata/", "/test-data/", "/fixtures/", "/fixture/",
        "/docs/", "/doc/", "/documentation/", "/docs_src/",
        "/e2e/", "/benchmark/", "/benchmarks/",
        "/website/", "/paradox/", "/i18n/",
    ];
    if always_noisy.iter().any(|d| p.contains(d)) {
        return true;
    }

    // Shallow-only noisy dirs: only filter when appearing within the first path component.
    // Depth is measured as the number of '/' in the prefix before the segment:
    //   0 slashes → e.g. `src/examples/…`          → filter
    //   1+ slashes → e.g. `packages/pkg/example/…`  → keep (monorepo sub-package)
    // This allows Java packages like org.springframework.samples.petclinic and
    // Dart monorepos like packages/riverpod_sqflite/example/ to be retrieved.
    let shallow_noisy = ["/example/", "/examples/", "/sample/", "/samples/"];
    for noisy in &shallow_noisy {
        if let Some(pos) = p.find(noisy) {
            let slash_count = p[..pos].matches('/').count();
            if slash_count < 1 {
                return true;
            }
        }
    }

    // Top-level noisy directories (no leading slash)
    let top_level = [
        "testdata/", "samples/", "examples/", "docs/", "doc/", "docs_src/",
        "fixtures/", "benchmarks/", "website/",
    ];
    if top_level.iter().any(|d| p.starts_with(d)) {
        return true;
    }

    // Directory names ending with "-docs" or "_docs" (e.g. "akka-docs/")
    if p.split('/').any(|seg| seg.ends_with("-docs") || seg.ends_with("_docs")) {
        return true;
    }

    false
}

/// Read the `signatures` column for a file from the DB.
fn get_signatures(conn: &Connection, path: &str) -> Result<String> {
    let sigs: String = conn.query_row(
        "SELECT COALESCE(signatures, '') FROM files WHERE path = ?1",
        [path],
        |row| row.get(0),
    )?;
    Ok(sigs)
}

/// Tokenize a free-text query into lowercase words.
fn tokenize_query(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| t.len() >= 2)
        .map(|t| t.to_lowercase())
        .collect()
}

/// Split a camelCase or PascalCase identifier into lowercase component words.
///
/// "componentEmits" → ["component", "emits"]
/// "RouterLink"     → ["router", "link"]
/// "onChange"       → ["on", "change"]
/// "HTTPRequest"    → ["http", "request"]   (consecutive-uppercase run breaks before lowercase)
fn split_camel_case(s: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = s.chars().collect();
    for (i, &ch) in chars.iter().enumerate() {
        if ch.is_uppercase() && !current.is_empty() {
            let next_is_lower = chars.get(i + 1).map_or(false, |c| c.is_lowercase());
            let prev_is_lower = current.chars().last().map_or(false, |c| c.is_lowercase());
            if prev_is_lower || next_is_lower {
                if current.len() >= 2 {
                    parts.push(current.to_lowercase());
                }
                current = ch.to_string();
            } else {
                current.push(ch);
            }
        } else {
            current.push(ch);
        }
    }
    if current.len() >= 2 {
        parts.push(current.to_lowercase());
    }
    parts
}

/// Tokenize signature text into a set of lowercase identifier tokens.
/// Expands camelCase/PascalCase identifiers so "componentEmits" also yields "component" + "emits".
fn tokenize_sigs(text: &str) -> HashSet<String> {
    let mut result = HashSet::new();
    for token in text.split(|c: char| !c.is_alphanumeric() && c != '_') {
        if token.len() < 2 { continue; }
        result.insert(token.to_lowercase());
        for part in split_camel_case(token) {
            if part.len() >= 2 {
                result.insert(part);
            }
        }
    }
    result
}

/// Tokenize a file path into component segments (e.g. "src/foo/barBaz.rs" → ["src","foo","barbaz","bar","baz"]).
/// Expands camelCase path components for Vue/Angular-style file names.
fn tokenize_path(path: &str) -> HashSet<String> {
    let mut result = HashSet::new();
    for token in path.split(|c: char| c == '/' || c == '\\' || c == '.' || c == '_' || c == '-') {
        if token.len() < 2 { continue; }
        result.insert(token.to_lowercase());
        for part in split_camel_case(token) {
            if part.len() >= 2 {
                result.insert(part);
            }
        }
    }
    result
}

/// Score lexical match between stored signatures + path and query tokens.
///
/// Weights mirror SigMap's defaults:
/// - Exact token in signatures: +1.0
/// - Token in file path:        +0.8
/// - Prefix match in sigs (≥4 chars): +0.3
///
/// Returns a score in [0, 1].
fn score_lexical(sigs: &str, path: &str, query_tokens: &[String]) -> f64 {
    if query_tokens.is_empty() {
        return 0.0;
    }
    let sig_toks = tokenize_sigs(sigs);
    let path_toks = tokenize_path(path);

    let raw: f64 = query_tokens
        .iter()
        .map(|t| {
            let mut s = 0.0_f64;
            if sig_toks.contains(t) {
                s += 1.0;
            }
            if path_toks.contains(t) {
                s += 0.8;
            }
            if t.len() >= 4 && (sig_toks.iter().any(|st| st.starts_with(t.as_str()))
                              || path_toks.iter().any(|pt| pt.starts_with(t.as_str()))) {
                s += 0.3;
            }
            s
        })
        .sum();

    // Normalise: max possible per token is 2.1 (exact + path + prefix)
    (raw / (query_tokens.len() as f64 * 2.1)).min(1.0)
}

/// Compute cosine similarity between two embeddings
fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }

    let min_len = a.len().min(b.len());
    let a = &a[..min_len];
    let b = &b[..min_len];

    let dot_product: f64 = a.iter().zip(b.iter()).map(|(x, y)| (*x as f64) * (*y as f64)).sum();

    let a_norm: f64 = a.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    let b_norm: f64 = b.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();

    if a_norm == 0.0 || b_norm == 0.0 {
        return 0.0;
    }

    dot_product / (a_norm * b_norm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let v = vec![1.0, 0.0, 0.0];
        let similarity = cosine_similarity(&v, &v);
        assert!((similarity - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let similarity = cosine_similarity(&a, &b);
        assert!(similarity.abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        let similarity = cosine_similarity(&a, &b);
        assert!((similarity + 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_blob_to_embedding() {
        let bytes = vec![0, 0, 128, 63]; // 1.0 in little-endian f32
        let embedding = blob_to_embedding(&bytes);
        assert_eq!(embedding.len(), 1);
        assert!((embedding[0] - 1.0).abs() < 1e-6);
    }
}
