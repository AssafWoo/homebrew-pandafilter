use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Analytics {
    pub input_tokens: usize,
    pub output_tokens: usize,
    pub savings_pct: f32,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub timestamp_secs: u64,
    /// e.g. "build" from "cargo build", "status" from "git status"
    #[serde(default)]
    pub subcommand: Option<String>,
    /// Wall-clock time the underlying command took to execute, in milliseconds
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// True when the result was served from the post-pipeline result cache.
    #[serde(default)]
    pub cache_hit: bool,
}

impl Analytics {
    pub fn compute(input_tokens: usize, output_tokens: usize) -> Self {
        Self::compute_with_command(input_tokens, output_tokens, None)
    }

    pub fn compute_with_command(
        input_tokens: usize,
        output_tokens: usize,
        command: Option<String>,
    ) -> Self {
        Self::new(input_tokens, output_tokens, command, None, None)
    }

    pub fn new(
        input_tokens: usize,
        output_tokens: usize,
        command: Option<String>,
        subcommand: Option<String>,
        duration_ms: Option<u64>,
    ) -> Self {
        let savings_pct = if input_tokens == 0 {
            0.0
        } else {
            let saved = input_tokens.saturating_sub(output_tokens);
            (saved as f32 / input_tokens as f32) * 100.0
        };
        let timestamp_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self {
            input_tokens,
            output_tokens,
            savings_pct,
            command,
            timestamp_secs,
            subcommand,
            duration_ms,
            cache_hit: false,
        }
    }

    pub fn new_cache_hit(
        input_tokens: usize,
        output_tokens: usize,
        command: Option<String>,
        subcommand: Option<String>,
    ) -> Self {
        let mut a = Self::new(input_tokens, output_tokens, command, subcommand, None);
        a.cache_hit = true;
        a
    }

    /// Tokens saved (absolute)
    pub fn tokens_saved(&self) -> usize {
        self.input_tokens.saturating_sub(self.output_tokens)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn savings_pct_correct() {
        let a = Analytics::compute(100, 60);
        assert!((a.savings_pct - 40.0).abs() < 0.001);
    }

    #[test]
    fn zero_input_tokens_no_panic() {
        let a = Analytics::compute(0, 0);
        assert_eq!(a.savings_pct, 0.0);
    }

    #[test]
    fn full_savings() {
        let a = Analytics::compute(100, 0);
        assert!((a.savings_pct - 100.0).abs() < 0.001);
    }

    #[test]
    fn cache_hit_flag_set() {
        let a = Analytics::new_cache_hit(100, 20, Some("cargo".to_string()), None);
        assert!(a.cache_hit);
        assert_eq!(a.input_tokens, 100);
    }
}
