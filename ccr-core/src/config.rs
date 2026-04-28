use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CcrConfig {
    #[serde(default)]
    pub global: GlobalConfig,
    #[serde(default)]
    pub commands: HashMap<String, CommandConfig>,
    #[serde(default)]
    pub tee: TeeConfig,
    #[serde(default)]
    pub read: ReadConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TeeConfig {
    pub enabled: bool,
    pub mode: TeeMode,
    pub max_files: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TeeMode {
    Aggressive,
    Always,
    Never,
}

impl Default for TeeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: TeeMode::Aggressive,
            max_files: 20,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ReadMode {
    #[default]
    Passthrough, // current behaviour — zero regression
    Auto,        // use auto_level() (>300→Aggressive, >100→Strip, else Passthrough)
    Strip,
    Aggressive,
    Structural,  // signatures with nesting context, collapsed bodies
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ReadConfig {
    #[serde(default)]
    pub mode: ReadMode,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct GlobalConfig {
    pub summarize_threshold_lines: usize,
    pub head_lines: usize,
    pub tail_lines: usize,
    pub strip_ansi: bool,
    pub normalize_whitespace: bool,
    pub deduplicate_lines: bool,
    /// Additional regex patterns for lines that must never be dropped.
    /// Each entry is ORed with the built-in critical pattern
    /// (error|warning|failed|fatal|panic|exception|critical).
    /// Example: ["OOMKilled", "timeout", "deadline exceeded"]
    #[serde(default)]
    pub hard_keep_patterns: Vec<String>,
    /// BERT embedding model to use for semantic summarization.
    /// Options: "AllMiniLML6V2" (default, ~90MB), "AllMiniLML12V2" (~120MB).
    /// First call wins — changing this requires restarting the process.
    #[serde(default = "default_bert_model")]
    pub bert_model: String,
    /// Commands whose output represents persistent system state.
    /// These get full-content storage in SessionEntry (no 4000-char cap),
    /// enabling accurate line-level delta across long state outputs.
    #[serde(default = "default_state_commands")]
    pub state_commands: Vec<String>,
    /// Override the cost per million input tokens used in `ccr gain`.
    /// If unset, CCR auto-detects from the ANTHROPIC_MODEL env var,
    /// falling back to $3.00/1M (Claude Sonnet 4.6).
    /// Example: cost_per_million_tokens = 15.0  # for Opus
    #[serde(default)]
    pub cost_per_million_tokens: Option<f64>,
    /// Hard ceiling on raw input bytes before any pipeline stage.
    /// 0 = disabled. Default 200_000 (~50K tokens).
    #[serde(default = "default_input_char_ceiling")]
    pub input_char_ceiling: usize,
    /// Hard cap on pipeline output chars. 0 = disabled. Default 50_000.
    #[serde(default = "default_output_char_cap")]
    pub output_char_cap: usize,
    /// Enable the MoE-inspired sparse filter router (opt-in, default false).
    /// When true, content features drive expert selection instead of the fixed pipeline.
    #[serde(default)]
    pub use_router: bool,
    /// When use_router=true, inject a small exploration bonus (+0.5) to underused
    /// experts when one expert exceeds 70% of activations. Prevents expert collapse.
    #[serde(default)]
    pub router_exploration_noise: bool,
    /// Unix nice level for the panda process. Higher = lower priority.
    /// Default 10. Set to 0 to disable.
    #[serde(default = "default_nice_level")]
    pub nice_level: i32,
    /// Number of ONNX Runtime intra-op threads. Default 2.
    /// Higher values speed up large batches but compete with the editor/LSP.
    #[serde(default = "default_ort_threads")]
    pub ort_threads: usize,
}

fn default_input_char_ceiling() -> usize {
    200_000
}

fn default_output_char_cap() -> usize {
    50_000
}

fn default_bert_model() -> String {
    "AllMiniLML6V2".to_string()
}

fn default_nice_level() -> i32 {
    10
}

fn default_ort_threads() -> usize {
    2
}

fn default_state_commands() -> Vec<String> {
    ["git", "kubectl", "ps", "ls", "df", "docker"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            summarize_threshold_lines: 50,
            head_lines: 30,
            tail_lines: 30,
            strip_ansi: true,
            normalize_whitespace: true,
            deduplicate_lines: true,
            hard_keep_patterns: Vec::new(),
            bert_model: default_bert_model(),
            state_commands: default_state_commands(),
            cost_per_million_tokens: None,
            input_char_ceiling: default_input_char_ceiling(),
            output_char_cap: default_output_char_cap(),
            use_router: false,
            router_exploration_noise: false,
            nice_level: default_nice_level(),
            ort_threads: default_ort_threads(),
        }
    }
}

impl CcrConfig {
    /// Return a copy of this config adjusted for the given context pressure.
    /// pressure: 0.0 = no change, 1.0 = maximum tightening.
    ///
    /// At p=1.0:
    ///   - summarize_threshold_lines shrinks to 25% of configured value (min 30)
    ///   - head_lines / tail_lines shrink to 40% of configured values (min 4 each)
    pub fn with_pressure(mut self, pressure: f32) -> Self {
        if pressure < 0.01 {
            return self;
        }
        let p = pressure.clamp(0.0, 1.0);
        let threshold_factor = 1.0 - 0.75 * p;
        self.global.summarize_threshold_lines = ((self.global.summarize_threshold_lines as f32
            * threshold_factor) as usize)
            .max(30);
        let budget_factor = 1.0 - 0.60 * p;
        self.global.head_lines =
            ((self.global.head_lines as f32 * budget_factor) as usize).max(4);
        self.global.tail_lines =
            ((self.global.tail_lines as f32 * budget_factor) as usize).max(4);
        self
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CommandConfig {
    #[serde(default)]
    pub patterns: Vec<FilterPattern>,
    /// Substitution returned when the entire output is blank after all filters.
    /// TOML: `on_empty = "(nothing to do)"`
    #[serde(default)]
    pub on_empty: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FilterPattern {
    pub regex: String,
    pub action: FilterAction,
    /// Strip ANSI escape codes from the line before applying this pattern's regex.
    /// The original (ANSI-carrying) line is preserved in the output unless removed.
    #[serde(default)]
    pub strip_ansi: bool,
}

/// 8-stage filter DSL — all variants are backward-compatible via `#[serde(untagged)]`.
///
/// TOML examples:
/// ```toml
/// action = "Remove"
/// action = "Collapse"
/// action = { ReplaceWith = "[npm install complete]" }
/// action = { TruncateLinesAt = 120 }
/// action = { HeadLines = 30 }
/// action = { TailLines = 30 }
/// action = { OnEmpty = "(nothing to do)" }
/// action = { MatchOutput = { message = "Build succeeded", unless = "error" } }
/// ```
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum FilterAction {
    // ── Existing (backward-compatible) ───────────────────────────────────────
    Simple(SimpleAction),
    #[allow(non_snake_case)]
    ReplaceWith { ReplaceWith: String },

    // ── New DSL stages ────────────────────────────────────────────────────────
    /// Short-circuit: if ANY line matches this pattern's regex, immediately
    /// return `message` instead of continuing through the filter chain.
    /// If `unless` is set and also matches any line, the short-circuit is suppressed.
    #[allow(non_snake_case)]
    MatchOutput { MatchOutput: MatchOutputConfig },

    /// Truncate each matching line to at most N characters (adds `…` suffix).
    #[allow(non_snake_case)]
    TruncateLinesAt { TruncateLinesAt: usize },

    /// After all line-level passes, keep only the first N lines.
    #[allow(non_snake_case)]
    HeadLines { HeadLines: usize },

    /// After all line-level passes, keep only the last N lines.
    #[allow(non_snake_case)]
    TailLines { TailLines: usize },

    /// If the output is blank after all processing, substitute this message.
    #[allow(non_snake_case)]
    OnEmpty { OnEmpty: String },
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct MatchOutputConfig {
    pub message: String,
    #[serde(default)]
    pub unless: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub enum SimpleAction {
    Remove,
    Collapse,
}
