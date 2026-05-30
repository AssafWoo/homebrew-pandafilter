//! Per-session state: cross-turn output cache and compression tracking.
//!
//! Session identity uses the parent PID of the Claude Code process, injected
//! by the hook script as `PANDA_SESSION_ID=$PPID`. Falls back to an hourly
//! timestamp window for `panda run` invocations from a terminal.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const MAX_ENTRIES: usize = 30;
const SIMILARITY_THRESHOLD: f32 = 0.92;

/// Max number of files stored in the per-session content cache for delta/structural mode.
const MAX_CACHE_FILES: usize = 20;
/// Max content bytes stored per cached file (20 KB).
const MAX_CACHE_FILE_BYTES: usize = 20_480;

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone)]
pub struct SessionEntry {
    pub turn: usize,
    pub cmd: String,
    pub ts: u64,
    pub tokens: usize,
    /// BERT embedding of the filtered output (384-dim).
    pub embedding: Vec<f32>,
    /// First 4000 chars of filtered output — used by delta compression and
    /// sentence-level dedup (C1). 4000 chars ≈ 30-50 lines.
    pub content_preview: String,
    /// Full content for state commands (git, kubectl, ps, etc.).
    /// Enables accurate line-level delta beyond the 4000-char preview boundary.
    #[serde(default)]
    pub state_content: Option<String>,
    /// Cosine similarity of this output's centroid vs the historical command centroid
    /// at record time. None on the first run or when BERT is skipped.
    #[serde(default)]
    pub centroid_delta: Option<f32>,
    /// Compact serialised error signatures for this entry.
    /// Used by error-loop detection to produce structural diffs across retries.
    /// Format: one `code|file|message` key per line (from ErrorSet::to_storage).
    #[serde(default)]
    pub error_signatures: Option<String>,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct SessionState {
    pub entries: Vec<SessionEntry>,
    /// Total tool-use turns seen in this session.
    pub total_turns: usize,
    /// Cumulative filtered tokens emitted in this session.
    pub total_tokens: usize,
    /// Per-command centroid: rolling mean of filtered-output embeddings for each command.
    /// Used by Idea 7 (historical centroid scoring) to measure what's genuinely new
    /// vs what a normal run of this command always produces.
    #[serde(default)]
    pub command_centroids: std::collections::HashMap<String, Vec<f32>>,
    /// 3.2: Paragraph-level embeddings of recently read file sections.
    /// Used for cross-file dedup — sections with >0.8 similarity to already-read
    /// content get collapsed to zoom blocks.
    #[serde(default)]
    pub read_section_embeddings: Vec<Vec<f32>>,
    /// 3.3: Recent edit locations per file (file_path → Vec<(start_line, end_line)>).
    /// Used to preserve context around recently-edited areas during re-reads.
    #[serde(default)]
    pub recent_edits: std::collections::HashMap<String, Vec<(usize, usize)>>,
    /// Per-file content cache for delta/structural read mode.
    /// Keys are absolute file paths; values are (mtime, content_snapshot).
    /// Capped at `MAX_CACHE_FILES` entries and `MAX_CACHE_FILE_BYTES` per file.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub file_content_cache: std::collections::HashMap<String, FileCacheEntry>,
}

/// Cached content of a file read earlier in this session.
/// Used by delta mode (send only what changed) and structural mode (send signatures).
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct FileCacheEntry {
    /// Unix timestamp of the file's mtime when it was cached.
    pub mtime_secs: u64,
    /// File content snapshot (capped at `MAX_CACHE_FILE_BYTES`).
    pub content: String,
}

pub struct SessionHit {
    pub turn: usize,
    pub age_secs: u64,
    /// Tokens that were saved by not re-emitting the full output.
    pub tokens_saved: usize,
}

// ── Session identity ──────────────────────────────────────────────────────────

/// Returns the stable session identifier for this Claude Code session.
///
/// The hook script injects `PANDA_SESSION_ID=$PPID` so that all hook invocations
/// within one Claude Code process share the same session file.
pub fn session_id() -> String {
    std::env::var("PANDA_SESSION_ID").unwrap_or_else(|_| {
        // Fallback: group by calendar day (UTC) so a long session spanning an
        // hour boundary doesn't get split into two separate state files.
        let secs = now_secs();
        format!("ts_{}", secs / 86400)
    })
}

// ── Persistence ───────────────────────────────────────────────────────────────

fn session_path(id: &str) -> Option<PathBuf> {
    Some(
        dirs::data_local_dir()?
            .join("panda")
            .join("sessions")
            .join(format!("{}.json", id)),
    )
}

impl SessionState {
    pub fn load(id: &str) -> Self {
        session_path(id)
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, id: &str) {
        if let Some(path) = session_path(id) {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(json) = serde_json::to_string(self) {
                let _ = std::fs::write(path, json);
            }
        }
    }
}

// ── Cross-turn similarity check (B3) ─────────────────────────────────────────

impl SessionState {
    /// Check if a recent run of the same command produced semantically identical output.
    /// Returns `Some(hit)` when cosine similarity exceeds the threshold.
    pub fn find_similar(&self, cmd: &str, embedding: &[f32]) -> Option<SessionHit> {
        self.find_similar_with_threshold(cmd, embedding, SIMILARITY_THRESHOLD)
    }

    /// Like `find_similar` but with a caller-supplied cosine similarity
    /// threshold (0.0–1.0). Used by Read dedup to scale the threshold by file size.
    pub fn find_similar_with_threshold(
        &self,
        cmd: &str,
        embedding: &[f32],
        threshold: f32,
    ) -> Option<SessionHit> {
        debug_assert!((0.0..=1.0).contains(&threshold), "threshold must be in [0.0, 1.0], got {}", threshold);
        let now = now_secs();
        self.entries
            .iter()
            .filter(|e| e.cmd == cmd && !e.embedding.is_empty())
            .rev()
            .find_map(|e| {
                let sim = cosine_sim(embedding, &e.embedding);
                if sim >= threshold {
                    Some(SessionHit {
                        turn: e.turn,
                        age_secs: now.saturating_sub(e.ts),
                        tokens_saved: e.tokens,
                    })
                } else {
                    None
                }
            })
    }

    /// Returns true if any entry for `cmd` was recorded within the last `window_secs`.
    /// Used to gate the polling suppressor without computing an embedding first.
    pub fn has_recent_entry(&self, cmd: &str, window_secs: u64) -> bool {
        let now = now_secs();
        self.entries
            .iter()
            .rev()
            .filter(|e| e.cmd == cmd && !e.embedding.is_empty())
            .any(|e| now.saturating_sub(e.ts) <= window_secs)
    }

    /// Like `find_similar` but restricted to entries within `RECENT_WINDOW_SECS`
    /// and using a lower similarity threshold (`RECENT_THRESHOLD`).
    ///
    /// Designed for the polling suppressor: in a tight loop (curl, gh, kubectl)
    /// the same status response recurs within seconds. The reduced threshold
    /// handles cases where only timestamps or counters differ between responses.
    pub fn find_similar_recent(&self, cmd: &str, embedding: &[f32]) -> Option<SessionHit> {
        const RECENT_WINDOW_SECS: u64 = 120;
        const RECENT_THRESHOLD: f32 = 0.80;
        let now = now_secs();
        self.entries
            .iter()
            .rev()
            .filter(|e| e.cmd == cmd && !e.embedding.is_empty())
            // take_while works because entries are pushed in ascending time order
            // and we reversed, so we walk newest-first and stop at first old entry.
            .take_while(|e| now.saturating_sub(e.ts) <= RECENT_WINDOW_SECS)
            .find_map(|e| {
                let sim = cosine_sim(embedding, &e.embedding);
                if sim >= RECENT_THRESHOLD {
                    Some(SessionHit {
                        turn: e.turn,
                        age_secs: now.saturating_sub(e.ts),
                        tokens_saved: e.tokens,
                    })
                } else {
                    None
                }
            })
    }

    /// Check if the most recent run of this command produced exactly the same
    /// output.  Used for state commands (git, kubectl, …) where semantic
    /// similarity is unreliable — two different states can have near-identical
    /// embeddings while the actual content has changed.
    pub fn find_exact(&self, cmd: &str, content: &str) -> Option<SessionHit> {
        let now = now_secs();
        self.entries
            .iter()
            .filter(|e| e.cmd == cmd)
            .rev()
            .find_map(|e| {
                let stored = e
                    .state_content
                    .as_deref()
                    .unwrap_or(&e.content_preview);
                if stored == content {
                    Some(SessionHit {
                        turn: e.turn,
                        age_secs: now.saturating_sub(e.ts),
                        tokens_saved: e.tokens,
                    })
                } else {
                    None
                }
            })
    }

    /// Record a new output entry, evicting the oldest if over capacity.
    /// `is_state`: if true (state commands like git, kubectl), stores the full
    /// content in `state_content` for accurate line-level delta beyond 4000 chars.
    /// `centroid_delta`: cosine similarity of this output's centroid vs historical
    /// at record time; `None` on first run or when BERT was skipped.
    pub fn record(
        &mut self,
        cmd: &str,
        embedding: Vec<f32>,
        tokens: usize,
        content: &str,
        is_state: bool,
        centroid_delta: Option<f32>,
    ) {
        self.total_turns += 1;
        self.total_tokens += tokens;

        const PREVIEW_CHARS: usize = 4_000;
        let (content_preview, state_content) = if is_state {
            (
                content.chars().take(PREVIEW_CHARS).collect(),
                Some(content.to_string()),
            )
        } else {
            (content.chars().take(PREVIEW_CHARS).collect(), None)
        };

        let entry = SessionEntry {
            turn: self.total_turns,
            cmd: cmd.to_string(),
            ts: now_secs(),
            tokens,
            embedding,
            content_preview,
            state_content,
            centroid_delta,
            error_signatures: None,
        };

        self.entries.push(entry);
        if self.entries.len() > MAX_ENTRIES {
            self.entries.remove(0);
        }
    }

    /// Find the most recent entry for `cmd` that has error signatures stored.
    /// Returns `(turn, signatures_storage_str)` for use by error-loop detection.
    pub fn find_error_loop(&self, cmd: &str) -> Option<(usize, &str)> {
        self.entries
            .iter()
            .rev()
            .find(|e| e.cmd == cmd && e.error_signatures.is_some())
            .and_then(|e| e.error_signatures.as_deref().map(|s| (e.turn, s)))
    }

    /// Update the most recent entry for `cmd` with error signatures.
    /// Called immediately after `record()` so the just-pushed entry is updated.
    pub fn set_last_error_signatures(&mut self, cmd: &str, sigs: String) {
        if let Some(entry) = self.entries.iter_mut().rev().find(|e| e.cmd == cmd) {
            entry.error_signatures = Some(sigs);
        }
    }

    /// Returns the centroid_delta from the most recent entry for `cmd` that has one.
    /// Used by adaptive pressure to detect stable (repetitive) command output.
    pub fn last_centroid_delta(&self, cmd: &str) -> Option<f32> {
        self.entries
            .iter()
            .rev()
            .find(|e| e.cmd == cmd && e.centroid_delta.is_some())
            .and_then(|e| e.centroid_delta)
    }
}

// ── Per-command historical centroid (Idea 7) ─────────────────────────────────

impl SessionState {
    /// Returns the rolling-mean centroid for this command, if any runs have been recorded.
    pub fn command_centroid(&self, cmd: &str) -> Option<&Vec<f32>> {
        self.command_centroids.get(cmd)
    }

    /// Update the per-command centroid with a new observation.
    /// Uses a simple running mean: new_centroid = (old * n + new) / (n + 1),
    /// where n is the number of prior entries for this command.
    pub fn update_command_centroid(&mut self, cmd: &str, new_centroid: Vec<f32>) {
        let prior_count = self.entries.iter().filter(|e| e.cmd == cmd).count() as f32;

        let updated = match self.command_centroids.get(cmd) {
            None => new_centroid,
            Some(_) if prior_count <= 0.0 => new_centroid,
            Some(old) => {
                let n = prior_count;
                let np1 = n + 1.0;
                old.iter()
                    .zip(new_centroid.iter())
                    .map(|(o, nc)| (o * n + nc) / np1)
                    .collect()
            }
        };

        self.command_centroids.insert(cmd.to_string(), updated);
    }
}

// ── Semantic delta compression (Idea 3) ──────────────────────────────────────

/// Threshold for delta matching: lower than B3 (0.92) so that iterative
/// workflows (cargo build N times) get delta treatment even when outputs are
/// only moderately similar.
const DELTA_THRESHOLD: f32 = 0.55;

/// Result of a semantic delta comparison between a new output and a prior run.
pub struct DeltaResult {
    /// Compressed output showing only new/changed lines plus a back-reference.
    pub output: String,
    /// Number of new output lines not semantically matched to the prior run.
    #[allow(dead_code)]
    pub new_count: usize,
    /// Number of new output lines matched (suppressed) by the prior run.
    #[allow(dead_code)]
    pub same_count: usize,
    /// Turn number of the prior run this delta references.
    #[allow(dead_code)]
    pub reference_turn: usize,
}

impl SessionState {
    /// Compare `new_lines` against recent entries for the same `cmd`.
    ///
    /// Returns `Some(DeltaResult)` when a prior run is found with overall
    /// embedding similarity ≥ DELTA_THRESHOLD.  Lines semantically matched
    /// to prior output are suppressed; genuinely new lines are surfaced.
    pub fn compute_delta(
        &self,
        cmd: &str,
        new_lines: &[&str],
        new_embedding: &[f32],
    ) -> Option<DeltaResult> {
        // Find the most recent entry for the same command within delta range
        let prior = self
            .entries
            .iter()
            .filter(|e| e.cmd == cmd && !e.embedding.is_empty())
            .rev()
            .find(|e| {
                let sim = cosine_sim(new_embedding, &e.embedding);
                sim >= DELTA_THRESHOLD
            })?;

        // Re-embed each new line and compare against the prior content.
        // Use state_content for full comparison when available (state commands),
        // otherwise fall back to content_preview.
        let model = panda_core::summarizer::embed_batch(new_lines).ok()?;

        let prior_text = prior
            .state_content
            .as_deref()
            .unwrap_or(&prior.content_preview);

        let prior_lines: Vec<&str> = prior_text.lines().collect();
        if prior_lines.is_empty() {
            return None;
        }
        let prior_embs = panda_core::summarizer::embed_batch(&prior_lines).ok()?;

        const LINE_MATCH_THRESHOLD: f32 = 0.88;
        let mut new_lines_out: Vec<String> = Vec::new();
        let mut same_count = 0usize;
        let mut new_count = 0usize;

        for (i, line) in new_lines.iter().enumerate() {
            let line_emb = &model[i];
            let best_sim = prior_embs
                .iter()
                .map(|pe| cosine_sim(line_emb, pe))
                .fold(0.0f32, f32::max);

            if best_sim >= LINE_MATCH_THRESHOLD {
                same_count += 1;
            } else {
                new_count += 1;
                new_lines_out.push((*line).to_string());
            }
        }

        // Approximate tokens saved by the repeated lines.
        let approx_saved = prior.tokens.saturating_mul(same_count)
            / prior_lines.len().max(1);
        let ref_marker = format!(
            "[Δ from turn {}: +{} new, {} repeated — ~{} tokens saved]",
            prior.turn, new_count, same_count, approx_saved
        );

        let mut output_parts: Vec<String> = Vec::new();
        if same_count > 0 {
            output_parts.push(ref_marker);
        }
        output_parts.extend(new_lines_out);

        Some(DeltaResult {
            output: output_parts.join("\n"),
            new_count,
            same_count,
            reference_turn: prior.turn,
        })
    }
}

// ── Staleness pressure ────────────────────────────────────────────────────────

impl SessionState {
    /// Compute additional compression pressure from stale session entries.
    ///
    /// Detects state-command outputs (git status, ls, …), build outputs that
    /// predate the last edit, and file reads of subsequently-edited files.
    /// The fraction of session tokens that are stale maps to additional pressure,
    /// capped at 0.3 so staleness never dominates the total pressure calculation.
    pub fn staleness_pressure(&self) -> f32 {
        let stale = crate::staleness::detect_stale_entries(self);
        if stale.is_empty() {
            return 0.0;
        }
        let stale_tokens: usize = stale
            .iter()
            .filter_map(|s| self.entries.iter().find(|e| e.turn == s.turn))
            .map(|e| e.tokens)
            .sum();
        let ratio = stale_tokens as f32 / self.total_tokens.max(1) as f32;
        // Scale: 50% of ratio → pressure, max 0.3
        (ratio * 0.5).min(0.3)
    }
}

// ── Session-aware compression budget (C2) ────────────────────────────────────

impl SessionState {
    /// Returns context pressure in [0.0, 1.0].
    /// 0.0 = fresh session (no extra compression needed).
    /// 1.0 = context is critically full (maximum tightening active).
    /// Ramps linearly from PRESSURE_START to PRESSURE_MAX cumulative output tokens.
    pub fn context_pressure(&self) -> f32 {
        const PRESSURE_START: usize = 25_000;
        const PRESSURE_MAX: usize = 80_000;
        if self.total_tokens <= PRESSURE_START {
            return 0.0;
        }
        let range = (PRESSURE_MAX - PRESSURE_START) as f32;
        let pos = self.total_tokens.saturating_sub(PRESSURE_START) as f32;
        (pos / range).min(1.0)
    }

    /// Returns a compression factor in [0.5, 1.0].
    ///
    /// At 1.0 (fresh session): no extra compression beyond the handler's own filter.
    /// Decreases linearly toward 0.5 once the session exceeds 50k cumulative tokens,
    /// signalling that the context window is filling up and outputs should be shorter.
    pub fn compression_factor(&self) -> f32 {
        const THRESHOLD: usize = 50_000;
        if self.total_tokens < THRESHOLD {
            return 1.0;
        }
        let excess = (self.total_tokens - THRESHOLD) as f32 / THRESHOLD as f32;
        (1.0 - 0.5 * excess.min(1.0)).max(0.5)
    }

    /// Returns content previews of the N most recent entries, oldest first.
    /// Used by the sentence-level deduplicator (C1) as the "prior context" window.
    pub fn recent_content(&self, limit: usize) -> Vec<(usize, String)> {
        self.entries
            .iter()
            .rev()
            .take(limit)
            .rev()
            .map(|e| (e.turn, e.content_preview.clone()))
            .collect()
    }
}

// ── Cross-file dedup (3.2) ────────────────────────────────────────────────

impl SessionState {
    /// Store paragraph-level embeddings from a recently read file.
    /// Keeps at most 200 section embeddings (FIFO eviction).
    pub fn add_read_section_embeddings(&mut self, embeddings: Vec<Vec<f32>>) {
        const MAX_SECTIONS: usize = 200;
        self.read_section_embeddings.extend(embeddings);
        let overflow = self.read_section_embeddings.len().saturating_sub(MAX_SECTIONS);
        if overflow > 0 {
            self.read_section_embeddings.drain(..overflow);
        }
    }

    /// Check if a section embedding is similar (>threshold) to any stored read section.
    pub fn is_section_seen(&self, emb: &[f32], threshold: f32) -> bool {
        self.read_section_embeddings
            .iter()
            .any(|stored| cosine_sim(emb, stored) >= threshold)
    }
}

// ── Edit tracking (3.3) ──────────────────────────────────────────────────

impl SessionState {
    /// Record that lines [start..end] of `file_path` were recently edited.
    /// Keeps at most 10 edit ranges per file (newest first).
    pub fn record_edit(&mut self, file_path: &str, start_line: usize, end_line: usize) {
        let ranges = self.recent_edits.entry(file_path.to_string()).or_default();
        ranges.push((start_line, end_line));
        if ranges.len() > 10 {
            ranges.remove(0);
        }
    }

    /// Get the set of line ranges that should be preserved uncompressed for a file.
    /// Returns ranges expanded by `context` lines on each side.
    pub fn edit_preserve_ranges(&self, file_path: &str, context: usize) -> Vec<(usize, usize)> {
        self.recent_edits
            .get(file_path)
            .map(|ranges| {
                ranges
                    .iter()
                    .map(|(s, e)| (s.saturating_sub(context), e + context))
                    .collect()
            })
            .unwrap_or_default()
    }
}

// ── File content cache (delta/structural mode) ───────────────────────────────

impl SessionState {
    /// Get the cached content for a file, if present.
    /// Returns `(mtime_secs, content)`.
    pub fn get_file_cache(&self, file_path: &str) -> Option<(u64, &str)> {
        self.file_content_cache
            .get(file_path)
            .map(|e| (e.mtime_secs, e.content.as_str()))
    }

    /// Store or update the cached content for a file.
    ///
    /// Enforces:
    /// - `MAX_CACHE_FILE_BYTES` per entry (content truncated at a line boundary)
    /// - `MAX_CACHE_FILES` total entries (evicts oldest insertion when full)
    pub fn set_file_cache(&mut self, file_path: &str, mtime_secs: u64, content: &str) {
        // Skip files that are too large even when truncated
        if content.len() > MAX_CACHE_FILE_BYTES * 2 {
            return;
        }

        // Truncate at line boundary if content exceeds per-file cap
        let stored_content = if content.len() <= MAX_CACHE_FILE_BYTES {
            content.to_string()
        } else {
            // Walk backwards from the limit to find the last newline
            let truncated = &content[..MAX_CACHE_FILE_BYTES];
            truncated
                .rfind('\n')
                .map(|pos| truncated[..=pos].to_string())
                .unwrap_or_else(|| truncated.to_string())
        };

        // Evict the entry with the oldest insertion (first key) if at capacity
        if self.file_content_cache.len() >= MAX_CACHE_FILES
            && !self.file_content_cache.contains_key(file_path)
        {
            if let Some(oldest_key) = self.file_content_cache.keys().next().cloned() {
                self.file_content_cache.remove(&oldest_key);
            }
        }

        self.file_content_cache.insert(
            file_path.to_string(),
            FileCacheEntry {
                mtime_secs,
                content: stored_content,
            },
        );
    }

    /// Invalidate the cache entry for `file_path` (called on Edit/Write).
    pub fn invalidate_file_cache(&mut self, file_path: &str) {
        self.file_content_cache.remove(file_path);
    }

    /// Returns the set of filenames currently "in focus" for this session.
    ///
    /// Focus is determined by two signals (both high-confidence):
    /// 1. Files with recorded edits (`recent_edits`) — strongest signal.
    /// 2. Files in the content cache (`file_content_cache`) — agent read them.
    ///
    /// Returns bare filenames (not full paths) so that e.g. `src/auth/jwt.rs`
    /// edited earlier causes `jwt.rs` to surface in build/lint output regardless
    /// of how the tool output refers to the path.
    pub fn focused_file_paths(&self) -> Vec<String> {
        let mut names: std::collections::HashSet<String> = std::collections::HashSet::new();

        for path in self.recent_edits.keys() {
            if let Some(fname) = std::path::Path::new(path).file_name() {
                if let Some(s) = fname.to_str() {
                    names.insert(s.to_string());
                }
            }
        }
        for path in self.file_content_cache.keys() {
            if let Some(fname) = std::path::Path::new(path).file_name() {
                if let Some(s) = fname.to_str() {
                    names.insert(s.to_string());
                }
            }
        }

        names.into_iter().collect()
    }
}

// ── Session digest for pre-compaction capture ─────────────────────────────────

/// Serialised session context injected after compaction to restore orientation.
pub struct SessionDigest {
    pub markdown: String,
}

impl SessionState {
    /// Build a human-readable Markdown digest of the current session.
    /// Captures: modified files with line ranges, recent error signatures,
    /// top commands by token cost, and context centroid notes.
    pub fn extract_digest(&self) -> SessionDigest {
        let mut md = String::from("## PandaFilter Session Digest (pre-compaction)\n\n");

        // Files modified this session
        if !self.recent_edits.is_empty() {
            md.push_str("### Files Modified\n");
            let mut files: Vec<(&String, &Vec<(usize, usize)>)> = self.recent_edits.iter().collect();
            files.sort_by_key(|(k, _)| k.as_str());
            for (path, ranges) in &files {
                let range_str: Vec<String> = ranges
                    .iter()
                    .map(|(s, e)| format!("L{}-{}", s, e))
                    .collect();
                md.push_str(&format!("- `{}` ({})\n", path, range_str.join(", ")));
            }
            md.push('\n');
        }

        // Recent error signatures (unique, most recent first)
        let error_sigs: Vec<&str> = self
            .entries
            .iter()
            .rev()
            .filter_map(|e| e.error_signatures.as_deref())
            .take(5)
            .collect();
        if !error_sigs.is_empty() {
            md.push_str("### Recent Error Signatures\n");
            for sig in error_sigs {
                for line in sig.lines().take(3) {
                    md.push_str(&format!("- {}\n", line));
                }
            }
            md.push('\n');
        }

        // Top commands by token cost
        if !self.entries.is_empty() {
            let mut cmd_tokens: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
            for e in &self.entries {
                *cmd_tokens.entry(e.cmd.as_str()).or_default() += e.tokens;
            }
            let mut cmd_list: Vec<(&&str, &usize)> = cmd_tokens.iter().collect();
            cmd_list.sort_by(|a, b| b.1.cmp(a.1));

            md.push_str("### Top Commands (by token cost)\n");
            for (cmd, tokens) in cmd_list.iter().take(8) {
                md.push_str(&format!("- `{}` — {} tokens\n", cmd, tokens));
            }
            md.push('\n');
        }

        // Session stats
        md.push_str(&format!(
            "### Session Stats\n- Total turns: {}\n- Total tokens: {}\n",
            self.total_turns, self.total_tokens
        ));

        SessionDigest { markdown: md }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    crate::handlers::util::cosine_similarity(a, b)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Human-readable age string, e.g. "3s", "2m", "1h".
pub fn format_age(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_session_with_entry(cmd: &str, embedding: Vec<f32>) -> SessionState {
        let mut s = SessionState::default();
        s.record(cmd, embedding, 100, "test content", false, None);
        s
    }

    #[test]
    fn find_similar_with_threshold_respects_threshold() {
        // Two nearly identical embeddings (cosine ~0.995)
        let emb_a = vec![1.0, 0.0, 0.0];
        let emb_b = vec![0.99, 0.1, 0.0];
        let session = make_session_with_entry("test", emb_a);

        // Low threshold: should match
        assert!(session.find_similar_with_threshold("test", &emb_b, 0.90).is_some());
        // Very high threshold: should not match
        assert!(session.find_similar_with_threshold("test", &emb_b, 0.999).is_none());
    }

    #[test]
    fn find_similar_with_threshold_different_cmd_no_match() {
        let emb = vec![1.0, 0.0, 0.0];
        let session = make_session_with_entry("git status", emb.clone());
        assert!(session.find_similar_with_threshold("git log", &emb, 0.5).is_none());
    }

    #[test]
    fn find_similar_delegates_to_threshold_variant() {
        let emb = vec![1.0, 0.0, 0.0];
        let session = make_session_with_entry("cmd", emb.clone());
        // find_similar uses SIMILARITY_THRESHOLD (0.92); identical embedding should match
        assert!(session.find_similar("cmd", &emb).is_some());
    }

    #[test]
    fn find_similar_empty_session_returns_none() {
        let session = SessionState::default();
        let emb = vec![1.0, 0.0, 0.0];
        assert!(session.find_similar_with_threshold("cmd", &emb, 0.5).is_none());
    }

    #[test]
    fn has_recent_entry_true_for_fresh_entry() {
        let emb = vec![1.0, 0.0, 0.0];
        let session = make_session_with_entry("curl", emb);
        // Entry was just recorded (ts = now), so window of 120s should match.
        assert!(session.has_recent_entry("curl", 120));
    }

    #[test]
    fn has_recent_entry_false_for_different_cmd() {
        let emb = vec![1.0, 0.0, 0.0];
        let session = make_session_with_entry("curl", emb);
        assert!(!session.has_recent_entry("gh", 120));
    }

    #[test]
    fn has_recent_entry_false_for_empty_session() {
        let session = SessionState::default();
        assert!(!session.has_recent_entry("curl", 120));
    }

    #[test]
    fn find_similar_recent_matches_identical_embedding() {
        let emb = vec![1.0, 0.0, 0.0];
        let session = make_session_with_entry("curl", emb.clone());
        // Identical embedding — should match at the 0.80 threshold.
        assert!(session.find_similar_recent("curl", &emb).is_some());
    }

    #[test]
    fn find_similar_recent_no_match_for_different_cmd() {
        let emb = vec![1.0, 0.0, 0.0];
        let session = make_session_with_entry("curl", emb.clone());
        assert!(session.find_similar_recent("gh", &emb).is_none());
    }

    #[test]
    fn find_similar_recent_no_match_below_threshold() {
        let emb_a = vec![1.0, 0.0, 0.0];
        let emb_b = vec![0.0, 1.0, 0.0]; // cosine sim = 0.0
        let session = make_session_with_entry("curl", emb_a);
        assert!(session.find_similar_recent("curl", &emb_b).is_none());
    }
}
