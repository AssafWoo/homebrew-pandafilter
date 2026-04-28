use once_cell::sync::OnceCell;
use regex::Regex;
use std::cell::RefCell;

static CRITICAL_PATTERN: OnceCell<Regex> = OnceCell::new();

fn critical_pattern() -> &'static Regex {
    CRITICAL_PATTERN.get_or_init(|| {
        Regex::new(r"(?i)(error|warning|warn|failed|failure|fatal|panic|exception|critical|FAILED|ERROR|WARNING)").unwrap()
    })
}

// ── P4: Configurable hard-keep patterns ───────────────────────────────────────

thread_local! {
    static EXTRA_KEEP_PATTERNS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Set additional regex patterns (beyond the built-in critical set) for lines
/// that must never be dropped during summarization.
/// Called once at startup from `main()` after loading config.
/// Thread-local — safe for the single-threaded hook path.
pub fn set_extra_keep_patterns(patterns: Vec<String>) {
    EXTRA_KEEP_PATTERNS.with(|p| {
        *p.borrow_mut() = patterns;
    });
}

/// Build the effective critical-line regex for the current call.
/// If no extra patterns are configured, returns a clone of the cached static.
/// Otherwise, ORs the extras into the pattern — called once per summarize invocation.
fn effective_critical_pattern() -> Regex {
    EXTRA_KEEP_PATTERNS.with(|p| {
        let extras = p.borrow();
        if extras.is_empty() {
            return critical_pattern().clone();
        }
        let valid_extras: Vec<String> = extras
            .iter()
            .filter(|e| !e.trim().is_empty())
            .map(|e| format!("(?:{})", e))
            .collect();
        if valid_extras.is_empty() {
            return critical_pattern().clone();
        }
        let pattern = format!("{}|{}", critical_pattern().as_str(), valid_extras.join("|"));
        Regex::new(&pattern).unwrap_or_else(|_| critical_pattern().clone())
    })
}

// ── P8: Configurable BERT model ───────────────────────────────────────────────

static MODEL_NAME: OnceCell<String> = OnceCell::new();

/// Set the BERT model name to use. Must be called before the first summarization.
/// First call wins (subsequent calls are no-ops).
/// Valid values: "AllMiniLML6V2" (default), "AllMiniLML12V2".
pub fn set_model_name(name: &str) {
    let _ = MODEL_NAME.set(name.to_string());
}

fn get_model_name() -> &'static str {
    MODEL_NAME.get().map(|s| s.as_str()).unwrap_or("AllMiniLML6V2")
}

// ── Cached model ──────────────────────────────────────────────────────────────

static MODEL_CACHE: OnceCell<fastembed::TextEmbedding> = OnceCell::new();

/// Sentinel file written after a successful model load/download.
/// Its presence means the model files are already on disk.
fn bert_sentinel() -> Option<std::path::PathBuf> {
    std::env::var("HOME").ok().map(|h| {
        std::path::PathBuf::from(h)
            .join(".local")
            .join("share")
            .join("ccr")
            .join(".bert_ready")
    })
}

fn bert_is_cached() -> bool {
    bert_sentinel().map(|p| p.exists()).unwrap_or(false)
}

fn mark_bert_cached() {
    if let Some(path) = bert_sentinel() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, "");
    }
}

fn load_model(name: &str) -> anyhow::Result<fastembed::TextEmbedding> {
    use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

    if !bert_is_cached() {
        eprintln!("[panda] downloading BERT model ({}, one-time setup)...", name);
        eprintln!("[panda] this may take a minute. future runs are instant.");
    }

    let embedding_model = match name {
        "AllMiniLML12V2" => EmbeddingModel::AllMiniLML12V2,
        _ => EmbeddingModel::AllMiniLML6V2,
    };

    let cache_dir = std::env::var("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".local/share/ccr/fastembed"))
        .unwrap_or_else(|_| std::path::PathBuf::from(".fastembed_cache"));

    let model = TextEmbedding::try_new(
        InitOptions::new(embedding_model)
            .with_cache_dir(cache_dir)
            .with_show_download_progress(false),
    )?;

    mark_bert_cached();
    Ok(model)
}

fn get_model() -> anyhow::Result<&'static fastembed::TextEmbedding> {
    MODEL_CACHE.get_or_try_init(|| load_model(get_model_name()))
}

/// Pre-warm the BERT model — downloads and caches it if not already present.
/// Called by `ccr init` so the download happens at setup time, not mid-session.
pub fn preload_model() -> anyhow::Result<()> {
    get_model()?;
    Ok(())
}

// ── Math helpers ──────────────────────────────────────────────────────────────

/// L2-normalize `v` in-place. No-op for zero vectors.
#[inline(always)]
fn l2_normalize(v: &mut Vec<f32>) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-9 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
}

/// Dot product of two slices. Equivalent to cosine similarity when both are L2-normalized.
#[inline(always)]
fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// Cosine similarity. All embeddings are L2-normalized at embedding time (via
/// `embed_and_normalize`), so this reduces to a plain dot product — no sqrt needed.
/// Clamped to [-1, 1] to absorb floating-point rounding errors that can push
/// the dot product of two near-identical unit vectors just above 1.0.
#[inline(always)]
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    dot(a, b).clamp(-1.0, 1.0)
}

/// Returns the value at `percentile` (0.0–1.0) of the score distribution.
/// `percentile = 0.70` → 70% of scores fall below the returned value,
/// so lines above it represent the top 30%.
fn score_percentile(scored: &[(usize, f32)], percentile: f32) -> f32 {
    if scored.is_empty() {
        return 0.0;
    }
    let mut vals: Vec<f32> = scored.iter().map(|(_, s)| *s).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((vals.len() as f32 * percentile) as usize).min(vals.len() - 1);
    vals[idx]
}

fn compute_centroid(embeddings: &[Vec<f32>]) -> Vec<f32> {
    if embeddings.is_empty() {
        return vec![];
    }
    let dim = embeddings[0].len();
    let mut centroid = vec![0.0f32; dim];
    for emb in embeddings {
        for (i, v) in emb.iter().enumerate() {
            centroid[i] += v;
        }
    }
    let n = embeddings.len() as f32;
    centroid.iter_mut().for_each(|v| *v /= n);
    // Normalize so the centroid is a unit vector; downstream similarity calls
    // can then use plain dot products instead of full cosine similarity.
    l2_normalize(&mut centroid);
    centroid
}

fn embed_and_normalize(texts: Vec<&str>) -> anyhow::Result<Vec<Vec<f32>>> {
    if let Some(embeddings) = crate::embed_client::daemon_embed(&texts, true) {
        return Ok(embeddings);
    }
    let model = get_model()?;
    let mut embeddings = model.embed(texts, None)?;
    for emb in &mut embeddings {
        l2_normalize(emb);
    }
    Ok(embeddings)
}

// ── Public result types ───────────────────────────────────────────────────────

pub struct SummarizeResult {
    pub output: String,
    pub lines_in: usize,
    pub lines_out: usize,
    pub omitted: usize,
}

// ── Line-level summarization (command output) ─────────────────────────────────

/// Build an omission marker, optionally embedding a Zoom-In expand ID.
fn make_omission_marker(count: usize, omitted: &[&str]) -> String {
    if crate::zoom::is_enabled() && count > 0 && !omitted.is_empty() {
        let id = crate::zoom::register(omitted.iter().map(|s| s.to_string()).collect());
        format!("[... {} lines omitted — ccr expand {} ...]", count, id)
    } else {
        format!("[... {} lines omitted ...]", count)
    }
}

/// Standard anomaly-based summarization: keeps outlier lines (errors, unique events)
/// and suppresses repetitive noise that clusters near the centroid.
pub fn summarize(text: &str, budget_lines: usize) -> SummarizeResult {
    let lines: Vec<&str> = text.lines().collect();
    let lines_in = lines.len();

    let output = match summarize_semantic(&lines, budget_lines, None) {
        Ok(result) => result,
        Err(_) => summarize_headtail(&lines, budget_lines),
    };

    let lines_out = output.lines().count();
    let omitted = lines_in.saturating_sub(lines_out);
    SummarizeResult { output, lines_in, lines_out, omitted }
}

/// Intent-aware summarization: blends command query (30%) with the user's current
/// task intent (70%) so that output lines relevant to what Claude is actually working
/// on score higher than lines that are merely relevant to the command name.
///
/// `command` — the raw command string (e.g. "cargo build")
/// `intent`  — last assistant message describing the current task goal
///
/// Falls back to `summarize_with_query(command)` when `intent` is empty.
pub fn summarize_with_intent(
    text: &str,
    budget_lines: usize,
    command: &str,
    intent: &str,
) -> SummarizeResult {
    if intent.trim().is_empty() {
        return summarize_with_query(text, budget_lines, command);
    }

    let lines: Vec<&str> = text.lines().collect();
    let lines_in = lines.len();

    let output = match summarize_semantic_intent(&lines, budget_lines, command, intent) {
        Ok(result) => result,
        Err(_) => summarize_headtail(&lines, budget_lines),
    };

    let lines_out = output.lines().count();
    let omitted = lines_in.saturating_sub(lines_out);
    SummarizeResult { output, lines_in, lines_out, omitted }
}

fn summarize_semantic_intent(
    lines: &[&str],
    budget: usize,
    command: &str,
    intent: &str,
) -> anyhow::Result<String> {
    let total = lines.len();
    let budget = budget.min(total);

    let indexed_lines: Vec<(usize, &str)> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| (i, *l))
        .collect();

    if indexed_lines.is_empty() {
        return Ok(lines.join("\n"));
    }


    // Embed lines + command + intent in one batch
    let mut texts: Vec<&str> = indexed_lines.iter().map(|(_, l)| *l).collect();
    texts.push(command);
    texts.push(intent);

    let all_embeddings = embed_and_normalize(texts)?;
    let n = indexed_lines.len();
    let cmd_emb = &all_embeddings[n];
    let intent_emb = &all_embeddings[n + 1];
    let embeddings = &all_embeddings[..n];

    // Blend: 30% command relevance + 70% intent relevance, then re-normalize
    // so the blended query remains a unit vector for correct dot-product similarity.
    let dim = cmd_emb.len();
    let mut blended_query: Vec<f32> = (0..dim)
        .map(|i| 0.30 * cmd_emb[i] + 0.70 * intent_emb[i])
        .collect();
    l2_normalize(&mut blended_query);

    let centroid = compute_centroid(embeddings);

    let scored: Vec<(usize, f32)> = indexed_lines
        .iter()
        .zip(embeddings.iter())
        .map(|((orig_idx, _), emb)| {
            let anomaly = 1.0 - cosine_similarity(emb, &centroid);
            let relevance = cosine_similarity(emb, &blended_query);
            (*orig_idx, 0.5 * anomaly + 0.5 * relevance)
        })
        .collect();

    // Hard-keep critical lines
    let critical = effective_critical_pattern();
    let mut selected: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (orig_idx, line) in lines.iter().enumerate() {
        if critical.is_match(line) {
            selected.insert(orig_idx);
        }
    }

    let max_score = scored.iter().map(|(_, s)| *s).fold(0.0f32, f32::max);
    let score_threshold = max_score * 0.30;

    let mut ranked = scored.clone();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    for (orig_idx, score) in &ranked {
        if selected.len() >= budget { break; }
        if *score < score_threshold { break; }
        selected.insert(*orig_idx);
    }

    let mut kept: Vec<usize> = selected.into_iter().collect();
    kept.sort();

    let mut result: Vec<String> = Vec::new();
    let mut prev_idx: Option<usize> = None;
    for idx in &kept {
        if let Some(prev) = prev_idx {
            let gap = idx - prev - 1;
            if gap > 0 {
                result.push(make_omission_marker(gap, &lines[prev + 1..*idx]));
            }
        } else if *idx > 0 {
            result.push(make_omission_marker(*idx, &lines[0..*idx]));
        }
        result.push(lines[*idx].to_string());
        prev_idx = Some(*idx);
    }
    if let Some(last) = prev_idx {
        let trailing = total - last - 1;
        if trailing > 0 {
            result.push(make_omission_marker(trailing, &lines[last + 1..]));
        }
    }

    Ok(result.join("\n"))
}

/// Query-biased summarization: combines anomaly scoring with relevance to `query`.
/// Lines that are both unusual AND relevant to the current task score highest.
pub fn summarize_with_query(text: &str, budget_lines: usize, query: &str) -> SummarizeResult {
    let lines: Vec<&str> = text.lines().collect();
    let lines_in = lines.len();

    let output = match summarize_semantic(&lines, budget_lines, Some(query)) {
        Ok(result) => result,
        Err(_) => summarize_headtail(&lines, budget_lines),
    };

    let lines_out = output.lines().count();
    let omitted = lines_in.saturating_sub(lines_out);
    SummarizeResult { output, lines_in, lines_out, omitted }
}

fn summarize_semantic(
    lines: &[&str],
    budget: usize,
    query: Option<&str>,
) -> anyhow::Result<String> {
    let total = lines.len();
    let budget = budget.min(total);

    let indexed_lines: Vec<(usize, &str)> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| (i, *l))
        .collect();

    if indexed_lines.is_empty() {
        return Ok(lines.join("\n"));
    }


    // Embed lines + optional query in one batch, L2-normalizing all vectors so
    // downstream similarity calls reduce to plain dot products.
    let mut texts: Vec<&str> = indexed_lines.iter().map(|(_, l)| *l).collect();
    let has_query = query.is_some();
    if let Some(q) = query {
        texts.push(q);
    }

    let all_embeddings = embed_and_normalize(texts)?;
    let query_emb: Option<&Vec<f32>> = if has_query { all_embeddings.last() } else { None };
    let embeddings = if has_query {
        &all_embeddings[..all_embeddings.len() - 1]
    } else {
        &all_embeddings[..]
    };

    let centroid = compute_centroid(embeddings);

    // Score: anomaly component always present, query relevance blended in when available
    let scored: Vec<(usize, f32)> = indexed_lines
        .iter()
        .zip(embeddings.iter())
        .map(|((orig_idx, _), emb)| {
            let anomaly = 1.0 - cosine_similarity(emb, &centroid);
            let score = if let Some(q_emb) = query_emb {
                let relevance = cosine_similarity(emb, q_emb);
                0.5 * anomaly + 0.5 * relevance
            } else {
                anomaly
            };
            (*orig_idx, score)
        })
        .collect();

    // Hard-keep critical lines
    let critical = effective_critical_pattern();
    let mut selected: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (orig_idx, line) in lines.iter().enumerate() {
        if critical.is_match(line) {
            selected.insert(orig_idx);
        }
    }

    // Fill budget from highest-scoring lines above threshold
    let max_score = scored.iter().map(|(_, s)| *s).fold(0.0f32, f32::max);
    // Slightly lower threshold in query-biased mode since relevance can shift scores
    let threshold_factor = if has_query { 0.30 } else { 0.40 };
    let score_threshold = max_score * threshold_factor;

    let mut ranked = scored.clone();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    for (orig_idx, score) in &ranked {
        if selected.len() >= budget {
            break;
        }
        if *score < score_threshold {
            break;
        }
        selected.insert(*orig_idx);
    }

    // Restore order, insert omission markers between gaps
    let mut kept: Vec<usize> = selected.into_iter().collect();
    kept.sort();

    let mut result: Vec<String> = Vec::new();
    let mut prev_idx: Option<usize> = None;
    for idx in &kept {
        if let Some(prev) = prev_idx {
            let gap = idx - prev - 1;
            if gap > 0 {
                result.push(make_omission_marker(gap, &lines[prev + 1..*idx]));
            }
        } else if *idx > 0 {
            result.push(make_omission_marker(*idx, &lines[0..*idx]));
        }
        result.push(lines[*idx].to_string());
        prev_idx = Some(*idx);
    }
    if let Some(last) = prev_idx {
        let trailing = total - last - 1;
        if trailing > 0 {
            result.push(make_omission_marker(trailing, &lines[last + 1..]));
        }
    }

    Ok(result.join("\n"))
}

fn summarize_headtail(lines: &[&str], budget: usize) -> String {
    let total = lines.len();
    let head = budget / 2;
    let tail = budget - head;
    let omitted = total.saturating_sub(head + tail);

    let mut result: Vec<String> = Vec::new();
    let head_end = head.min(total);
    result.extend(lines[..head_end].iter().map(|l| l.to_string()));
    if omitted > 0 {
        let tail_start = total.saturating_sub(tail).max(head_end);
        result.push(make_omission_marker(omitted, &lines[head_end..tail_start]));
    }
    if tail > 0 && total > head {
        result.extend(lines[total.saturating_sub(tail)..].iter().map(|l| l.to_string()));
    }
    result.join("\n")
}

// ── Contextual anchoring ──────────────────────────────────────────────────────

/// Anomaly-based summarization with contextual anchoring.
///
/// After selecting the high-anomaly lines to keep, also keeps up to
/// `anchor_neighbors` semantically nearest lines for each anomaly, so that
/// errors appear alongside the function signatures / file pointers that give
/// them context. `anchor_neighbors = 0` is identical to plain `summarize`.
pub fn summarize_with_anchoring(
    text: &str,
    budget_lines: usize,
    anchor_neighbors: usize,
) -> SummarizeResult {
    summarize_with_anchoring_preembedded(text, budget_lines, anchor_neighbors, None)
}

/// Like [`summarize_with_anchoring`] but reuses `embeddings` pre-computed by a prior
/// noise-scoring pass (one entry per non-empty line in `text`, in order).
/// If the length doesn't match, falls back to re-embedding.
pub fn summarize_with_anchoring_preembedded(
    text: &str,
    budget_lines: usize,
    anchor_neighbors: usize,
    precomputed: Option<Vec<Vec<f32>>>,
) -> SummarizeResult {
    let lines: Vec<&str> = text.lines().collect();
    let lines_in = lines.len();

    if lines_in == 0 {
        return SummarizeResult { output: String::new(), lines_in: 0, lines_out: 0, omitted: 0 };
    }

    let output = match summarize_semantic_anchored(&lines, budget_lines, anchor_neighbors, precomputed) {
        Ok(s) => s,
        Err(_) => summarize_headtail(&lines, budget_lines),
    };

    let lines_out = output.lines().count();
    let omitted = lines_in.saturating_sub(lines_out);
    SummarizeResult { output, lines_in, lines_out, omitted }
}

fn summarize_semantic_anchored(
    lines: &[&str],
    budget: usize,
    anchor_neighbors: usize,
    precomputed: Option<Vec<Vec<f32>>>,
) -> anyhow::Result<String> {
    let total = lines.len();
    let budget = budget.min(total);

    let indexed_lines: Vec<(usize, &str)> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| (i, *l))
        .collect();

    if indexed_lines.is_empty() {
        return Ok(lines.join("\n"));
    }

    let n = indexed_lines.len();
    let embeddings: Vec<Vec<f32>> =
        if let Some(pre) = precomputed.filter(|p| p.len() == n) {
            pre
        } else {
            let texts: Vec<&str> = indexed_lines.iter().map(|(_, l)| *l).collect();
            embed_and_normalize(texts)?
        };

    let centroid = compute_centroid(&embeddings);

    let scored: Vec<(usize, f32)> = indexed_lines
        .iter()
        .zip(embeddings.iter())
        .map(|((orig_idx, _), emb)| (*orig_idx, 1.0 - cosine_similarity(emb, &centroid)))
        .collect();

    // Hard-keep critical lines
    let critical = effective_critical_pattern();
    let mut selected: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (orig_idx, line) in lines.iter().enumerate() {
        if critical.is_match(line) {
            selected.insert(orig_idx);
        }
    }

    let max_score = scored.iter().map(|(_, s)| *s).fold(0.0f32, f32::max);
    let score_threshold = max_score * 0.40;

    let mut ranked = scored.clone();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // First pass: select anomalous lines up to budget
    let mut anomaly_selected: std::collections::HashSet<usize> = selected.clone();
    for (orig_idx, score) in &ranked {
        if anomaly_selected.len() >= budget { break; }
        if *score < score_threshold { break; }
        anomaly_selected.insert(*orig_idx);
    }

    // Second pass: for each anomaly-selected line, find its nearest neighbors
    // and add them as context anchors (up to anchor_neighbors per anomaly line).
    if anchor_neighbors > 0 {
        // Build a map from orig_idx to embedding index in `indexed_lines`
        let orig_to_emb_idx: std::collections::HashMap<usize, usize> = indexed_lines
            .iter()
            .enumerate()
            .map(|(i, (orig, _))| (*orig, i))
            .collect();

        let anomaly_orig: Vec<usize> = anomaly_selected.iter().copied().collect();
        for anom_orig in &anomaly_orig {
            if let Some(&anom_emb_idx) = orig_to_emb_idx.get(anom_orig) {
                let anom_emb = &embeddings[anom_emb_idx];

                // Score all non-selected lines by similarity to this anomaly
                let mut candidates: Vec<(usize, f32)> = indexed_lines
                    .iter()
                    .enumerate()
                    .filter(|(_, (orig, _))| !anomaly_selected.contains(orig))
                    .map(|(i, (orig, _))| (*orig, cosine_similarity(&embeddings[i], anom_emb)))
                    .collect();
                candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

                for (orig_idx, _) in candidates.iter().take(anchor_neighbors) {
                    if selected.len() + anomaly_selected.len() >= budget * 2 {
                        break;
                    }
                    anomaly_selected.insert(*orig_idx);
                }
            }
        }
    }

    selected = anomaly_selected;

    let mut kept: Vec<usize> = selected.into_iter().collect();
    kept.sort();

    let mut result: Vec<String> = Vec::new();
    let mut prev_idx: Option<usize> = None;
    for idx in &kept {
        if let Some(prev) = prev_idx {
            let gap = idx - prev - 1;
            if gap > 0 {
                result.push(make_omission_marker(gap, &lines[prev + 1..*idx]));
            }
        } else if *idx > 0 {
            result.push(make_omission_marker(*idx, &lines[0..*idx]));
        }
        result.push(lines[*idx].to_string());
        prev_idx = Some(*idx);
    }
    if let Some(last) = prev_idx {
        let trailing = total - last - 1;
        if trailing > 0 {
            result.push(make_omission_marker(trailing, &lines[last + 1..]));
        }
    }

    Ok(result.join("\n"))
}

// ── Semantic entropy & entropy-adjusted budget ────────────────────────────────

/// Computes the mean distance of each embedding from the centroid.
/// Returns 0.0 for empty or single-element input.
/// High entropy → diverse content; low entropy → near-uniform/repetitive content.
pub fn semantic_entropy(embeddings: &[Vec<f32>]) -> f32 {
    if embeddings.len() <= 1 {
        return 0.0;
    }
    let centroid = compute_centroid(embeddings);
    let mean_dist: f32 = embeddings
        .iter()
        .map(|e| 1.0 - cosine_similarity(e, &centroid))
        .sum::<f32>()
        / embeddings.len() as f32;
    mean_dist
}

/// Returns a line budget adjusted by the semantic entropy of the input text.
///
/// Low-entropy input (repetitive/uniform lines) receives a tightly reduced budget
/// even early in the session — no need to show 50 near-identical "downloading…"
/// lines when 3 would convey the same information.
///
/// High-entropy input (diverse errors, warnings, types) receives the full budget.
///
/// If the input is below the summarize threshold (200 lines by default), the
/// full `max_budget` is returned unchanged.
pub fn entropy_adjusted_budget(text: &str, max_budget: usize) -> usize {
    const THRESHOLD_LINES: usize = 200;
    const LOW_ENTROPY_CUTOFF: f32 = 0.10;  // below this → max compression
    const HIGH_ENTROPY_CUTOFF: f32 = 0.35; // above this → full budget

    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() < THRESHOLD_LINES {
        return max_budget;
    }

    // Sample up to 100 lines evenly to avoid O(N²) cost on huge inputs
    let step = (lines.len() / 100).max(1);
    let sample: Vec<&str> = lines.iter().step_by(step).copied().collect();

    let embeddings = match embed_and_normalize(sample) {
        Ok(e) => e,
        Err(_) => return max_budget,
    };

    let entropy = semantic_entropy(&embeddings);

    // Linear interpolation between low and high cutoffs
    if entropy <= LOW_ENTROPY_CUTOFF {
        // Maximally repetitive — collapse to ~5% of budget (min 1)
        ((max_budget as f32 * 0.05) as usize).max(1)
    } else if entropy >= HIGH_ENTROPY_CUTOFF {
        max_budget
    } else {
        // Interpolate: 5% → 100% of max_budget
        let t = (entropy - LOW_ENTROPY_CUTOFF) / (HIGH_ENTROPY_CUTOFF - LOW_ENTROPY_CUTOFF);
        let fraction = 0.05 + t * 0.95;
        ((max_budget as f32 * fraction) as usize).max(1)
    }
}

/// Like [`entropy_adjusted_budget`] but operates on pre-computed L2-normalized embeddings,
/// skipping a second BERT pass when embeddings are already available from the noise-filter step.
///
/// This is the path taken by the pipeline when `noise_filter_with_embeddings` has already
/// embedded the surviving lines — we reuse those vectors rather than re-embedding a text sample.
pub fn entropy_adjusted_budget_preembedded(embeddings: &[Vec<f32>], max_budget: usize) -> usize {
    const THRESHOLD_LINES: usize = 200;
    const LOW_ENTROPY_CUTOFF: f32 = 0.10;
    const HIGH_ENTROPY_CUTOFF: f32 = 0.35;

    if embeddings.len() < THRESHOLD_LINES {
        return max_budget;
    }

    // Sample up to 100 embeddings evenly — mirrors the sampling in `entropy_adjusted_budget`.
    let step = (embeddings.len() / 100).max(1);
    let sample: Vec<Vec<f32>> = embeddings.iter().step_by(step).cloned().collect();

    let entropy = semantic_entropy(&sample);

    if entropy <= LOW_ENTROPY_CUTOFF {
        ((max_budget as f32 * 0.05) as usize).max(1)
    } else if entropy >= HIGH_ENTROPY_CUTOFF {
        max_budget
    } else {
        let t = (entropy - LOW_ENTROPY_CUTOFF) / (HIGH_ENTROPY_CUTOFF - LOW_ENTROPY_CUTOFF);
        let fraction = 0.05 + t * 0.95;
        ((max_budget as f32 * fraction) as usize).max(1)
    }
}

// ── Sentence-level summarization (conversation messages) ─────────────────────

pub struct MessageSummarizeResult {
    pub output: String,
    pub sentences_in: usize,
    pub sentences_out: usize,
}

pub fn summarize_message(text: &str, budget_ratio: f32) -> MessageSummarizeResult {
    let sentences = crate::sentence::split_sentences(text);
    let sentences_in = sentences.len();

    if sentences_in == 0 {
        return MessageSummarizeResult {
            output: text.to_string(),
            sentences_in: 0,
            sentences_out: 0,
        };
    }

    let budget = ((sentences_in as f32 * budget_ratio).ceil() as usize).max(1);
    if sentences_in <= budget {
        return MessageSummarizeResult {
            output: text.to_string(),
            sentences_in,
            sentences_out: sentences_in,
        };
    }

    let output = match summarize_sentences_semantic(&sentences, budget, is_hard_keep_sentence) {
        Ok(out) => out,
        Err(_) => summarize_sentences_headtail(&sentences, budget),
    };

    let sentences_out = crate::sentence::split_sentences(&output).len();
    MessageSummarizeResult { output, sentences_in, sentences_out }
}

pub fn summarize_assistant_message(text: &str, budget_ratio: f32) -> MessageSummarizeResult {
    let sentences = crate::sentence::split_sentences(text);
    let sentences_in = sentences.len();

    if sentences_in == 0 {
        return MessageSummarizeResult {
            output: text.to_string(),
            sentences_in: 0,
            sentences_out: 0,
        };
    }

    let budget = ((sentences_in as f32 * budget_ratio).ceil() as usize).max(1);
    if sentences_in <= budget {
        return MessageSummarizeResult {
            output: text.to_string(),
            sentences_in,
            sentences_out: sentences_in,
        };
    }

    let output = match summarize_sentences_semantic(&sentences, budget, is_hard_keep_assistant_sentence) {
        Ok(out) => out,
        Err(_) => summarize_sentences_headtail(&sentences, budget),
    };

    let sentences_out = crate::sentence::split_sentences(&output).len();
    MessageSummarizeResult { output, sentences_in, sentences_out }
}

fn is_hard_keep_sentence(s: &str) -> bool {
    let t = s.trim();
    if t.ends_with('?') { return true; }
    if t.contains('`') || t.contains("::") { return true; }
    if t.split_whitespace().any(|w| {
        let w = w.trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
        w.contains('_') && w.chars().next().map(|c| c.is_alphabetic()).unwrap_or(false)
    }) { return true; }
    let lower = t.to_lowercase();
    ["must", "never", "always", "ensure", "make sure", "do not", "don't", "avoid", "required", "critical"]
        .iter()
        .any(|kw| lower.contains(kw))
}

fn is_hard_keep_assistant_sentence(s: &str) -> bool {
    let t = s.trim();
    if t.contains('`') || t.contains("::") { return true; }
    let first = t.chars().next().unwrap_or(' ');
    if first == '-' || first == '*' { return true; }
    if first.is_ascii_digit() && t.chars().nth(1).map(|c| c == '.' || c == ')').unwrap_or(false) {
        return true;
    }
    if t.contains('$') || t.contains('€') || t.contains('£') || t.contains('%') { return true; }
    if t.split_whitespace().any(|w| w.chars().any(|c| c.is_ascii_digit())) { return true; }
    let lower = t.to_lowercase();
    ["must", "never", "always", "ensure", "required", "critical"]
        .iter()
        .any(|kw| lower.contains(kw))
}

fn summarize_sentences_semantic(
    sentences: &[String],
    budget: usize,
    hard_keep: impl Fn(&str) -> bool,
) -> anyhow::Result<String> {
    let texts: Vec<&str> = sentences.iter().map(|s| s.as_str()).collect();
    let embeddings = embed_and_normalize(texts)?;

    let centroid = compute_centroid(&embeddings);

    let scored: Vec<(usize, f32)> = embeddings
        .iter()
        .enumerate()
        .map(|(i, emb)| (i, 1.0 - cosine_similarity(emb, &centroid)))
        .collect();

    let mut selected: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (i, s) in sentences.iter().enumerate() {
        if hard_keep(s) {
            selected.insert(i);
        }
    }

    let max_score = scored.iter().map(|(_, s)| *s).fold(0.0f32, f32::max);
    let threshold = max_score * 0.40;

    let mut ranked = scored.clone();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    for (idx, score) in &ranked {
        if selected.len() >= budget { break; }
        if *score < threshold { break; }
        selected.insert(*idx);
    }

    let mut kept: Vec<usize> = selected.into_iter().collect();
    kept.sort();
    Ok(kept.iter().map(|&i| sentences[i].clone()).collect::<Vec<_>>().join(" "))
}

fn summarize_sentences_headtail(sentences: &[String], budget: usize) -> String {
    let total = sentences.len();
    let head = budget / 2;
    let tail = budget - head;
    let mut result: Vec<String> = Vec::new();
    result.extend_from_slice(&sentences[..head.min(total)]);
    if total > head {
        let tail_start = total.saturating_sub(tail);
        if tail_start > head {
            result.extend_from_slice(&sentences[tail_start..]);
        }
    }
    result.join(" ")
}

// ── Semantic line clustering ──────────────────────────────────────────────────

const CLUSTER_SIM_THRESHOLD: f32 = 0.85;

/// Cluster-based summarization: groups near-duplicate lines into clusters and
/// keeps one representative per cluster plus a `[N similar]` marker.
/// Critical lines (errors/warnings) are always represented in the output.
pub fn summarize_with_clustering(text: &str, budget_lines: usize) -> SummarizeResult {
    summarize_with_clustering_preembedded(text, budget_lines, None)
}

/// Like [`summarize_with_clustering`] but reuses `embeddings` pre-computed by a prior
/// noise-scoring pass (one entry per non-empty line in `text`, in order).
/// If the length doesn't match, falls back to re-embedding.
pub fn summarize_with_clustering_preembedded(
    text: &str,
    budget_lines: usize,
    precomputed: Option<Vec<Vec<f32>>>,
) -> SummarizeResult {
    let lines: Vec<&str> = text.lines().collect();
    let lines_in = lines.len();

    if lines_in == 0 {
        return SummarizeResult { output: String::new(), lines_in: 0, lines_out: 0, omitted: 0 };
    }

    let output = match do_cluster_summarize(&lines, budget_lines, precomputed) {
        Ok(s) => s,
        Err(_) => summarize_headtail(&lines, budget_lines),
    };

    let lines_out = output.lines().count();
    let omitted = lines_in.saturating_sub(lines_out);
    SummarizeResult { output, lines_in, lines_out, omitted }
}

fn do_cluster_summarize(
    lines: &[&str],
    budget: usize,
    precomputed: Option<Vec<Vec<f32>>>,
) -> anyhow::Result<String> {
    let indexed: Vec<(usize, &str)> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| (i, *l))
        .collect();

    if indexed.is_empty() {
        return Ok(lines.join("\n"));
    }

    let n = indexed.len();
    let embeddings: Vec<Vec<f32>> =
        if let Some(pre) = precomputed.filter(|p| p.len() == n) {
            pre
        } else {
            let texts: Vec<&str> = indexed.iter().map(|(_, l)| *l).collect();
            embed_and_normalize(texts)?
        };

    // ── Greedy clustering ────────────────────────────────────────────────────
    // Each line joins the nearest cluster within threshold, or starts a new one.
    let mut member_cluster: Vec<usize> = vec![0; n];
    let mut centroids: Vec<Vec<f32>> = Vec::new();
    let mut sizes: Vec<usize> = Vec::new();

    for i in 0..n {
        let emb = &embeddings[i];
        let best = (0..centroids.len())
            .map(|cid| (cid, cosine_similarity(emb, &centroids[cid])))
            .filter(|(_, sim)| *sim >= CLUSTER_SIM_THRESHOLD)
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        match best {
            Some((cid, _)) => {
                let count = sizes[cid] as f32;
                let nc = count + 1.0;
                for (j, v) in emb.iter().enumerate() {
                    centroids[cid][j] = (centroids[cid][j] * count + v) / nc;
                }
                // Re-normalize the running centroid (spherical k-means) so it stays a
                // unit vector; downstream dot products remain valid cosine similarities.
                l2_normalize(&mut centroids[cid]);
                sizes[cid] += 1;
                member_cluster[i] = cid;
            }
            None => {
                let cid = centroids.len();
                centroids.push(emb.clone());
                sizes.push(1);
                member_cluster[i] = cid;
            }
        }
    }

    let num_clusters = centroids.len();

    // ── Pick best representative per cluster (highest anomaly = most distinct) ─
    let mut reps: Vec<Option<(usize, f32)>> = vec![None; num_clusters];
    for i in 0..n {
        let cid = member_cluster[i];
        let score = 1.0 - cosine_similarity(&embeddings[i], &centroids[cid]);
        match reps[cid] {
            None => reps[cid] = Some((i, score)),
            Some((_, best)) if score > best => reps[cid] = Some((i, score)),
            _ => {}
        }
    }

    // (orig_idx, cluster_size, anomaly_score, is_critical)
    let cluster_info: Vec<(usize, usize, f32, bool)> = (0..num_clusters)
        .map(|cid| {
            let (mi, score) = reps[cid].unwrap_or_else(|| {
                let first = (0..n).find(|&i| member_cluster[i] == cid).unwrap_or(0);
                (first, 0.0)
            });
            let orig_idx = indexed[mi].0;
            let is_crit = effective_critical_pattern().is_match(indexed[mi].1);
            (orig_idx, sizes[cid], score, is_crit)
        })
        .collect();

    // ── Budget selection ─────────────────────────────────────────────────────
    // Critical cluster reps are always kept; non-critical sorted by score.
    let critical: Vec<_> = cluster_info.iter().filter(|c| c.3).collect();
    let mut normal: Vec<_> = cluster_info.iter().filter(|c| !c.3).collect();
    normal.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

    let normal_budget = budget.saturating_sub(critical.len());
    let mut selected: Vec<(usize, usize)> = Vec::new(); // (orig_idx, cluster_size)
    for c in &critical {
        selected.push((c.0, c.1));
    }
    for c in normal.iter().take(normal_budget) {
        selected.push((c.0, c.1));
    }

    selected.sort_by_key(|(idx, _)| *idx);

    // ── Build output ─────────────────────────────────────────────────────────
    let mut result: Vec<String> = Vec::new();
    for (orig_idx, size) in &selected {
        result.push(lines[*orig_idx].to_string());
        if *size > 1 {
            result.push(format!("[{} similar]", size - 1));
        }
    }

    Ok(result.join("\n"))
}

// ── Historical-centroid summarization (Idea 7) ───────────────────────────────

/// Anomaly-based summarization scored against a **historical** centroid rather
/// than the current batch's centroid.
///
/// Lines that diverge from what this command *usually* produces are kept;
/// lines that look like every other run are suppressed. Critical lines are
/// always kept. Falls back to plain `summarize` when `historical_centroid` is
/// all-zeros (empty/uninitialized).
pub fn summarize_against_centroid(
    text: &str,
    budget_lines: usize,
    historical_centroid: &[f32],
) -> SummarizeResult {
    let lines: Vec<&str> = text.lines().collect();
    let lines_in = lines.len();

    if lines_in == 0 {
        return SummarizeResult { output: String::new(), lines_in: 0, lines_out: 0, omitted: 0 };
    }

    // Fall back to plain summarize if centroid is empty or all-zeros
    let centroid_magnitude: f32 = historical_centroid.iter().map(|v| v * v).sum::<f32>().sqrt();
    if historical_centroid.is_empty() || centroid_magnitude < 1e-6 {
        return summarize(text, budget_lines);
    }

    let output = match summarize_against_centroid_inner(&lines, budget_lines, historical_centroid) {
        Ok(s) => s,
        Err(_) => summarize_headtail(&lines, budget_lines),
    };

    let lines_out = output.lines().count();
    let omitted = lines_in.saturating_sub(lines_out);
    SummarizeResult { output, lines_in, lines_out, omitted }
}

fn summarize_against_centroid_inner(
    lines: &[&str],
    budget: usize,
    historical_centroid: &[f32],
) -> anyhow::Result<String> {
    let total = lines.len();
    let budget = budget.min(total);

    let indexed_lines: Vec<(usize, &str)> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| (i, *l))
        .collect();

    if indexed_lines.is_empty() {
        return Ok(lines.join("\n"));
    }

    let texts: Vec<&str> = indexed_lines.iter().map(|(_, l)| *l).collect();
    let embeddings = embed_and_normalize(texts)?;

    // Normalize the historical centroid at the point of use so it is compatible with
    // the unit-length line embeddings regardless of when it was stored (before or after
    // the pre-normalization optimization was introduced).
    let mut norm_centroid = historical_centroid.to_vec();
    l2_normalize(&mut norm_centroid);

    // Score anomaly against the historical centroid (not the current batch's centroid)
    let scored: Vec<(usize, f32)> = indexed_lines
        .iter()
        .zip(embeddings.iter())
        .map(|((orig_idx, _), emb)| {
            let anomaly = 1.0 - cosine_similarity(emb, &norm_centroid);
            (*orig_idx, anomaly)
        })
        .collect();

    // Hard-keep critical lines
    let critical = effective_critical_pattern();
    let mut selected: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (orig_idx, line) in lines.iter().enumerate() {
        if critical.is_match(line) {
            selected.insert(orig_idx);
        }
    }

    let max_score = scored.iter().map(|(_, s)| *s).fold(0.0f32, f32::max);
    let relative_threshold = max_score * 0.40;
    // Absolute floor: lines with anomaly below 0.10 are considered "normal for this command"
    // and suppressed regardless of budget. This handles the case where ALL lines are
    // near-identical to the historical centroid (max_score ≈ 0 → relative threshold ≈ 0).
    let absolute_floor: f32 = 0.10;
    let score_threshold = relative_threshold.max(absolute_floor);

    let mut ranked = scored.clone();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    for (orig_idx, score) in &ranked {
        if selected.len() >= budget { break; }
        if *score < score_threshold { break; }
        selected.insert(*orig_idx);
    }

    let mut kept: Vec<usize> = selected.into_iter().collect();
    kept.sort();

    let mut result: Vec<String> = Vec::new();
    let mut prev_idx: Option<usize> = None;
    for idx in &kept {
        if let Some(prev) = prev_idx {
            let gap = idx - prev - 1;
            if gap > 0 {
                result.push(make_omission_marker(gap, &lines[prev + 1..*idx]));
            }
        } else if *idx > 0 {
            result.push(make_omission_marker(*idx, &lines[0..*idx]));
        }
        result.push(lines[*idx].to_string());
        prev_idx = Some(*idx);
    }
    if let Some(last) = prev_idx {
        let trailing = total - last - 1;
        if trailing > 0 {
            result.push(make_omission_marker(trailing, &lines[last + 1..]));
        }
    }

    Ok(result.join("\n"))
}

// ── Zero-shot noise classification ───────────────────────────────────────────

/// Prototype strings that anchor the "useful" and "noise" poles in embedding space.
const USEFUL_PROTOTYPE: &str =
    "error message stack trace type mismatch test failure file path function signature warning";
const NOISE_PROTOTYPE: &str =
    "compiling downloading resolving fetching progress elapsed already up to date artifact";

static USEFUL_EMB: OnceCell<Vec<f32>> = OnceCell::new();
static NOISE_EMB: OnceCell<Vec<f32>> = OnceCell::new();

fn useful_embedding() -> anyhow::Result<&'static Vec<f32>> {
    USEFUL_EMB.get_or_try_init(|| {
        Ok(embed_and_normalize(vec![USEFUL_PROTOTYPE])?.remove(0))
    })
}

fn noise_embedding() -> anyhow::Result<&'static Vec<f32>> {
    NOISE_EMB.get_or_try_init(|| {
        Ok(embed_and_normalize(vec![NOISE_PROTOTYPE])?.remove(0))
    })
}

/// Scores each line as `useful_similarity - noise_similarity`.
///
/// Positive score  → line resembles useful developer output (errors, warnings, traces).
/// Negative score  → line resembles boilerplate noise (progress, artifacts, downloads).
///
/// Returns one score per input line in the same order. Empty input returns empty vec.
pub fn noise_scores(lines: &[&str]) -> anyhow::Result<Vec<f32>> {
    if lines.is_empty() {
        return Ok(vec![]);
    }

    let useful_emb = useful_embedding()?;
    let noise_emb = noise_embedding()?;

    let embeddings = embed_and_normalize(lines.to_vec())?;

    let scores = embeddings
        .iter()
        .map(|emb| cosine_similarity(emb, useful_emb) - cosine_similarity(emb, noise_emb))
        .collect();

    Ok(scores)
}

/// Combined noise filtering that returns the surviving text AND the BERT embeddings
/// for surviving non-empty lines.
///
/// Embeds only non-empty lines (consistent with the clustering/anchoring summarizers),
/// applies the same -0.05 noise threshold as [`noise_scores`], and returns:
/// - The filtered text (same as what the pipeline would keep after calling `noise_scores`)
/// - A `Vec<Vec<f32>>` with one embedding per non-empty line in that filtered text, in order
///
/// Passing these embeddings to [`summarize_with_clustering_preembedded`] or
/// [`summarize_with_anchoring_preembedded`] avoids a second full model.embed() call,
/// roughly halving BERT latency for every summarized output.
///
/// Returns `(original_text_as_vec, empty_vec)` if no lines survive filtering or if the
/// model is unavailable, so the caller always gets a valid result.
pub fn noise_filter_with_embeddings(
    lines: &[&str],
) -> anyhow::Result<(Vec<String>, Vec<Vec<f32>>)> {
    if lines.is_empty() {
        return Ok((vec![], vec![]));
    }

    // Only embed non-empty lines — same filter the summarizers apply
    let non_empty: Vec<(usize, &str)> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| (i, *l))
        .collect();

    if non_empty.is_empty() {
        return Ok((lines.iter().map(|s| s.to_string()).collect(), vec![]));
    }

    let useful_emb = useful_embedding()?;
    let noise_emb = noise_embedding()?;

    let texts: Vec<&str> = non_empty.iter().map(|(_, l)| *l).collect();
    let embeddings = embed_and_normalize(texts)?;

    // Mark noisy lines for removal
    let mut drop_set: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (j, &(orig_i, _)) in non_empty.iter().enumerate() {
        let score = cosine_similarity(&embeddings[j], useful_emb)
            - cosine_similarity(&embeddings[j], noise_emb);
        if score < -0.05 {
            drop_set.insert(orig_i);
        }
    }

    // Surviving lines (preserving empty lines that were not scored)
    let surviving_lines: Vec<String> = lines
        .iter()
        .enumerate()
        .filter(|(i, _)| !drop_set.contains(i))
        .map(|(_, l)| l.to_string())
        .collect();

    // Embeddings for surviving non-empty lines only, in order
    let surviving_embeddings: Vec<Vec<f32>> = non_empty
        .iter()
        .zip(embeddings.into_iter())
        .filter(|((orig_i, _), _)| !drop_set.contains(orig_i))
        .map(|(_, emb)| emb)
        .collect();

    Ok((surviving_lines, surviving_embeddings))
}

// ── Batch embedding (public) ──────────────────────────────────────────────────

pub fn embed_batch(texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
    embed_and_normalize(texts.to_vec())
}

/// Compute the mean embedding of all non-empty lines in `text`.
/// Used to update per-command historical centroids with line-mean quality
/// rather than embedding the whole text as a single string.
/// Returns a zero vector if the input has no non-empty lines.
pub fn compute_output_centroid(text: &str) -> anyhow::Result<Vec<f32>> {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return Ok(vec![0.0f32; 384]);
    }
    let embeddings = embed_batch(&lines)?;
    let dim = embeddings[0].len();
    let mut centroid = vec![0.0f32; dim];
    for emb in &embeddings {
        for (c, v) in centroid.iter_mut().zip(emb.iter()) {
            *c += v;
        }
    }
    let n = embeddings.len() as f32;
    centroid.iter_mut().for_each(|c| *c /= n);
    l2_normalize(&mut centroid);
    Ok(centroid)
}

/// Compute a normalized mean centroid from pre-computed L2-normalized embeddings.
/// Returns a 384-dim zero vector when the slice is empty.
/// Avoids a redundant embed_batch call when embeddings are already available.
pub fn compute_centroid_from_embeddings(embeddings: &[Vec<f32>]) -> Vec<f32> {
    if embeddings.is_empty() {
        return vec![0.0f32; 384];
    }
    let dim = embeddings[0].len();
    let mut centroid = vec![0.0f32; dim];
    for emb in embeddings {
        for (c, v) in centroid.iter_mut().zip(emb.iter()) {
            *c += v;
        }
    }
    let n = embeddings.len() as f32;
    centroid.iter_mut().for_each(|c| *c /= n);
    l2_normalize(&mut centroid);
    centroid
}

/// Compute semantic similarity between two texts. Used as a quality gate on generative output.
pub fn semantic_similarity(a: &str, b: &str) -> anyhow::Result<f32> {
    let embeddings = embed_and_normalize(vec![a, b])?;
    Ok(dot(&embeddings[0], &embeddings[1]))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_input_not_summarized() {
        let lines: Vec<String> = (0..50).map(|i| format!("line {}", i)).collect();
        let input = lines.join("\n");
        let result = summarize(&input, 60);
        assert!(result.lines_out <= 50 + 1);
    }

    #[test]
    fn long_input_summarized() {
        let lines: Vec<String> = (0..500).map(|i| format!("line {}", i)).collect();
        let input = lines.join("\n");
        let result = summarize(&input, 60);
        assert!(result.output.contains("lines omitted"));
        assert!(result.output.lines().count() < 500);
    }

    #[test]
    fn error_lines_always_kept() {
        let mut lines: Vec<String> = (0..250).map(|i| format!("noise line {}", i)).collect();
        lines[125] = "error[E0308]: mismatched types".to_string();
        let input = lines.join("\n");
        let result = summarize(&input, 60);
        assert!(result.output.contains("error[E0308]: mismatched types"));
    }

    #[test]
    fn warning_lines_always_kept() {
        let mut lines: Vec<String> = (0..250).map(|i| format!("noise line {}", i)).collect();
        lines[200] = "warning: unused variable `x`".to_string();
        let input = lines.join("\n");
        let result = summarize(&input, 60);
        assert!(result.output.contains("warning: unused variable `x`"));
    }

    #[test]
    fn single_line_input() {
        let result = summarize("just one line", 60);
        assert!(result.output.contains("just one line"));
    }

    #[test]
    fn omission_line_counts_correctly() {
        let lines: Vec<String> = (0..500).map(|i| format!("line {}", i)).collect();
        let input = lines.join("\n");
        let result = summarize(&input, 60);
        assert!(result.output.contains("lines omitted"));
    }

    #[test]
    fn configurable_budget() {
        let lines: Vec<String> = (0..100).map(|i| format!("line {}", i)).collect();
        let input = lines.join("\n");
        let result = summarize(&input, 10);
        assert!(result.output.lines().count() <= 100);
    }

    // ── Math helper unit tests ────────────────────────────────────────────────

    #[test]
    fn l2_normalize_produces_unit_vector() {
        let mut v = vec![3.0f32, 4.0];
        l2_normalize(&mut v);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6, "norm was {}", norm);
        // Direction preserved: 3/5 = 0.6, 4/5 = 0.8
        assert!((v[0] - 0.6).abs() < 1e-6);
        assert!((v[1] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn l2_normalize_zero_vector_is_noop() {
        let mut v = vec![0.0f32, 0.0, 0.0];
        l2_normalize(&mut v);
        assert_eq!(v, vec![0.0, 0.0, 0.0]);
    }

    #[test]
    fn dot_orthogonal_vectors_is_zero() {
        let a = vec![1.0f32, 0.0];
        let b = vec![0.0f32, 1.0];
        assert!((dot(&a, &b)).abs() < 1e-9);
    }

    #[test]
    fn dot_identical_unit_vectors_is_one() {
        let mut v = vec![3.0f32, 4.0];
        l2_normalize(&mut v);
        assert!((dot(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn score_percentile_70th() {
        // scores: 0.0, 0.1, 0.2, ..., 0.9  (10 values)
        let scored: Vec<(usize, f32)> = (0..10).map(|i| (i, i as f32 / 10.0)).collect();
        let p = score_percentile(&scored, 0.70);
        // 70th percentile of 10 values → index 7 → value 0.7
        assert!((p - 0.7).abs() < 1e-5, "p70 was {}", p);
    }

    #[test]
    fn score_percentile_empty_returns_zero() {
        let empty: Vec<(usize, f32)> = vec![];
        assert_eq!(score_percentile(&empty, 0.70), 0.0);
    }

    #[test]
    fn score_percentile_single_element() {
        let scored = vec![(0usize, 0.5f32)];
        assert!((score_percentile(&scored, 0.70) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn entropy_budget_preembedded_returns_valid_range() {
        // Diverse embeddings (orthogonal unit vectors) → high entropy → full budget.
        let dim = 8usize;
        let embeddings: Vec<Vec<f32>> = (0..250)
            .map(|i| {
                let mut v = vec![0.0f32; dim];
                v[i % dim] = 1.0;
                v
            })
            .collect();
        let max_budget = 60;
        let budget = entropy_adjusted_budget_preembedded(&embeddings, max_budget);
        assert!(budget >= 1, "budget was 0");
        assert!(budget <= max_budget, "budget {} > max {}", budget, max_budget);
    }

    #[test]
    fn entropy_budget_preembedded_identical_embeddings_low_budget() {
        // All identical embeddings → zero entropy → max compression (~5% of budget).
        let unit = vec![1.0f32, 0.0, 0.0, 0.0];
        let embeddings: Vec<Vec<f32>> = (0..250).map(|_| unit.clone()).collect();
        let max_budget = 60;
        let budget = entropy_adjusted_budget_preembedded(&embeddings, max_budget);
        // Should produce ~5% of max_budget (≥1 due to .max(1))
        assert!(budget >= 1);
        assert!(budget <= (max_budget as f32 * 0.10) as usize + 1,
            "expected low budget for uniform input, got {}", budget);
    }

    #[test]
    fn compute_centroid_is_unit_vector() {
        let embeddings = vec![
            vec![1.0f32, 0.0],
            vec![0.0f32, 1.0],
        ];
        let c = compute_centroid(&embeddings);
        let norm: f32 = c.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "centroid norm was {}", norm);
    }

    #[test]
    fn compute_centroid_from_embeddings_empty_returns_zero_vector() {
        let result = compute_centroid_from_embeddings(&[]);
        assert_eq!(result.len(), 384);
        assert!(result.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn compute_centroid_from_embeddings_single_is_normalized() {
        // A non-normalized vector should come back as a unit vector.
        let raw = vec![3.0f32, 4.0f32];
        // Pad to 384 dims with zero to simulate an actual embedding.
        let mut v = vec![0.0f32; 384];
        v[0] = 3.0;
        v[1] = 4.0;
        let result = compute_centroid_from_embeddings(&[v]);
        let norm: f32 = result.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "norm was {}", norm);
        // Direction should be preserved: ratio of first two components should be 3:4
        assert!((result[0] / result[1] - 3.0 / 4.0).abs() < 1e-5);
    }

    #[test]
    fn compute_centroid_from_embeddings_matches_compute_output_centroid() {
        // For a short, known input: both functions should produce the same centroid.
        let text = "hello world\nfoo bar baz\nrust is great";
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        let embeddings = embed_batch(&lines).expect("embed_batch failed");
        let from_pre = compute_centroid_from_embeddings(&embeddings);
        let from_text = compute_output_centroid(text)
            .expect("compute_output_centroid failed");
        // Should be numerically identical (both take the mean of per-line embeddings).
        for (a, b) in from_pre.iter().zip(from_text.iter()) {
            assert!((a - b).abs() < 1e-5, "mismatch at dim: {} vs {}", a, b);
        }
    }
}
