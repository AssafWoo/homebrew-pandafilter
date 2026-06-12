//! Compression feedback loop (FB).
//!
//! Every zoom block is an implicit question: "was it safe to collapse this?"
//! `panda expand ZI_N` is the agent answering *no* — it needed the content.
//! A block that is never expanded is a (weak) *yes*.
//!
//! This module aggregates those signals per command into a Beta posterior
//! over the expansion rate and converts it into a keep-threshold scale for
//! the summarizer: commands whose blocks keep getting expanded are
//! over-compressed (scale < 1.0 → keep more lines); commands whose blocks
//! are never expanded across many observations can be compressed slightly
//! harder (scale > 1.0).
//!
//! Storage: `~/.local/share/panda/feedback.json`. Load-modify-save without
//! locking — events are single-user telemetry and a lost increment is
//! harmless.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Target expansion rate: ~5% of blocks being expanded is healthy curiosity;
/// well above it means real information is being hidden.
const TARGET_RATE: f32 = 0.05;
/// Beta prior (α=1, β=19) → prior mean = TARGET_RATE. The prior's weight of
/// 20 pseudo-blocks means roughly 20 real observations are needed before the
/// scale moves meaningfully in either direction.
const PRIOR_ALPHA: f32 = 1.0;
const PRIOR_BETA: f32 = 19.0;
/// Sensitivity of the scale to deviation from the target rate.
const SLOPE: f32 = 1.5;
/// Bounds keep a single command's history from ever disabling summarization
/// (low side) or silently dropping critical content (high side).
const MIN_SCALE: f32 = 0.85;
const MAX_SCALE: f32 = 1.10;

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct CmdFeedback {
    /// Zoom blocks created for this command.
    pub blocks: u64,
    /// Zoom blocks of this command later expanded via `panda expand`.
    pub expands: u64,
}

#[derive(Serialize, Deserialize, Default)]
pub struct FeedbackStore {
    pub commands: HashMap<String, CmdFeedback>,
}

fn store_path() -> Option<PathBuf> {
    Some(dirs::data_local_dir()?.join("panda").join("feedback.json"))
}

impl FeedbackStore {
    pub fn load() -> Self {
        store_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Some(path) = store_path() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(json) = serde_json::to_string(self) {
                let _ = std::fs::write(path, json);
            }
        }
    }
}

/// Normalize a raw command string to its feedback key: the first two tokens
/// ("cargo test", "git diff"), so flags and file arguments don't fragment the
/// statistics into single-observation buckets.
pub fn command_key(cmd: &str) -> String {
    cmd.split_whitespace().take(2).collect::<Vec<_>>().join(" ")
}

/// Record that `n` zoom blocks were just created for `cmd`.
pub fn record_blocks(cmd: &str, n: u64) {
    if n == 0 {
        return;
    }
    let key = command_key(cmd);
    if key.is_empty() {
        return;
    }
    let mut store = FeedbackStore::load();
    store.commands.entry(key).or_default().blocks += n;
    store.save();
}

/// Record that a zoom block belonging to `cmd` was expanded.
pub fn record_expand(cmd: &str) {
    let key = command_key(cmd);
    if key.is_empty() {
        return;
    }
    let mut store = FeedbackStore::load();
    store.commands.entry(key).or_default().expands += 1;
    store.save();
}

/// Posterior-mean expansion rate for the given observation counts.
fn posterior_rate(blocks: u64, expands: u64) -> f32 {
    // Expands can briefly exceed blocks (e.g. the same block expanded twice);
    // clamp so the Beta parameters stay valid.
    let expands = expands.min(blocks.max(expands));
    (expands as f32 + PRIOR_ALPHA) / (blocks as f32 + PRIOR_ALPHA + PRIOR_BETA)
}

fn scale_for_rate(rate: f32) -> f32 {
    (1.0 - (rate - TARGET_RATE) * SLOPE).clamp(MIN_SCALE, MAX_SCALE)
}

/// Keep-threshold scale for `cmd`, derived from its observed expansion rate.
/// 1.0 with no data (the prior mean equals the target rate); < 1.0 when the
/// agent keeps expanding this command's blocks; > 1.0 (capped) when many
/// blocks have accumulated with no expansions.
pub fn keep_scale(cmd: &str) -> f32 {
    let store = FeedbackStore::load();
    keep_scale_from(&store, cmd)
}

fn keep_scale_from(store: &FeedbackStore, cmd: &str) -> f32 {
    let key = command_key(cmd);
    match store.commands.get(&key) {
        Some(fb) => scale_for_rate(posterior_rate(fb.blocks, fb.expands)),
        None => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_key_takes_first_two_tokens() {
        assert_eq!(command_key("cargo test --all -q"), "cargo test");
        assert_eq!(command_key("ls"), "ls");
        assert_eq!(command_key(""), "");
    }

    #[test]
    fn no_data_means_neutral_scale() {
        let store = FeedbackStore::default();
        assert!((keep_scale_from(&store, "cargo build") - 1.0).abs() < 1e-6);
    }

    #[test]
    fn prior_alone_is_neutral() {
        // 0 blocks, 0 expands → posterior mean = prior mean = target → scale 1.0
        let rate = posterior_rate(0, 0);
        assert!((rate - TARGET_RATE).abs() < 1e-6);
        assert!((scale_for_rate(rate) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn high_expansion_rate_lowers_scale() {
        // 20 blocks, 10 expanded → clearly over-compressed
        let mut store = FeedbackStore::default();
        store
            .commands
            .insert("cargo test".into(), CmdFeedback { blocks: 20, expands: 10 });
        let scale = keep_scale_from(&store, "cargo test --all");
        assert!(scale < 1.0, "scale was {}", scale);
        assert!(scale >= MIN_SCALE);
    }

    #[test]
    fn zero_expansions_with_many_blocks_raises_scale() {
        let mut store = FeedbackStore::default();
        store
            .commands
            .insert("npm install".into(), CmdFeedback { blocks: 200, expands: 0 });
        let scale = keep_scale_from(&store, "npm install --silent");
        assert!(scale > 1.0, "scale was {}", scale);
        assert!(scale <= MAX_SCALE);
    }

    #[test]
    fn few_observations_barely_move_scale() {
        // 3 blocks, 0 expands — prior dominates, scale stays near 1.0
        let mut store = FeedbackStore::default();
        store
            .commands
            .insert("go test".into(), CmdFeedback { blocks: 3, expands: 0 });
        let scale = keep_scale_from(&store, "go test ./...");
        assert!((scale - 1.0).abs() < 0.02, "scale was {}", scale);
    }

    #[test]
    fn scale_always_within_bounds() {
        for blocks in [0u64, 1, 10, 100, 10_000] {
            for expands in [0u64, 1, 50, 10_000] {
                let s = scale_for_rate(posterior_rate(blocks, expands));
                assert!((MIN_SCALE..=MAX_SCALE).contains(&s), "scale {} out of bounds", s);
            }
        }
    }
}
