use crate::analytics::Analytics;
use crate::ansi::strip_ansi;
use crate::config::CcrConfig;
use crate::patterns::PatternFilter;
use crate::summarizer::{
    entropy_adjusted_budget, noise_scores, summarize_against_centroid, summarize_with_anchoring,
    summarize_with_clustering, summarize_with_intent, summarize_with_query,
};
use crate::tokens::count_tokens;
use crate::whitespace::normalize;

/// Inputs above this line count are split into chunks for BERT processing,
/// reducing peak memory usage. Each chunk is summarized independently.
const CHUNK_THRESHOLD_LINES: usize = 2000;
/// Lines per chunk when chunked processing is active.
const CHUNK_SIZE_LINES: usize = 500;

pub struct PipelineResult {
    pub output: String,
    pub analytics: Analytics,
    /// Zoom-In blocks accumulated during this pipeline run.
    /// Each block holds the original lines from a collapsed/omitted group,
    /// keyed by the ZI_N ID embedded in the output marker.
    /// Empty when zoom is not enabled (e.g., `ccr filter`).
    pub zoom_blocks: Vec<crate::zoom::ZoomBlock>,
}

pub struct Pipeline {
    pub config: CcrConfig,
}

impl Pipeline {
    pub fn new(config: CcrConfig) -> Self {
        Self { config }
    }

    /// Process output through the pipeline.
    /// `command_hint` selects command-specific pattern rules.
    /// `query` biases BERT importance scoring toward task-relevant lines when provided.
    /// `historical_centroid` — when `Some`, scoring is done against what this command
    ///   *usually* produces, so only genuinely new/anomalous lines are kept.
    pub fn process(
        &self,
        input: &str,
        command_hint: Option<&str>,
        query: Option<&str>,
        historical_centroid: Option<&[f32]>,
    ) -> anyhow::Result<PipelineResult> {
        let input_tokens = count_tokens(input);

        let mut text = input.to_string();

        // 1. Strip ANSI
        if self.config.global.strip_ansi {
            text = strip_ansi(&text);
        }

        // 2. Normalize whitespace
        if self.config.global.normalize_whitespace {
            text = normalize(&text, &self.config.global);
        }

        // 3. Apply command-specific patterns
        if let Some(hint) = command_hint {
            if let Some(cmd_config) = self.config.commands.get(hint) {
                let filter = PatternFilter::new(cmd_config)?;
                text = filter.apply(&text);
            }
        }

        // 4. Summarize if too long
        if text.lines().count() > self.config.global.summarize_threshold_lines {
            let max_budget = self.config.global.head_lines + self.config.global.tail_lines;

            // 4a. Pre-filter noise (progress/download/compiling lines)
            {
                let lines: Vec<&str> = text.lines().collect();
                if let Ok(scores) = noise_scores(&lines) {
                    let filtered: Vec<&str> = lines
                        .iter()
                        .zip(scores.iter())
                        .filter_map(|(line, &score)| if score >= -0.05 { Some(*line) } else { None })
                        .collect();
                    if filtered.len() < lines.len() {
                        text = filtered.join("\n");
                    }
                }
            }

            // 4b. Only summarize if still over threshold after noise removal
            if text.lines().count() > self.config.global.summarize_threshold_lines {
                // Entropy-adaptive budget: diverse content gets more lines
                let budget = entropy_adjusted_budget(&text, max_budget);

                // 4c. Context-aware summarizer selection.
                // For very large inputs, split into chunks to reduce peak memory.
                let line_count = text.lines().count();
                text = if line_count > CHUNK_THRESHOLD_LINES {
                    self.summarize_chunked(&text, budget, command_hint, query, historical_centroid)
                } else {
                    self.summarize_single(&text, budget, command_hint, query, historical_centroid)
                };
            }
        }

        let output_tokens = count_tokens(&text);
        let analytics = Analytics::compute(input_tokens, output_tokens);

        Ok(PipelineResult { output: text, analytics, zoom_blocks: crate::zoom::drain() })
    }

    /// Summarize a single block of text using the context-aware strategy.
    /// Priority: centroid (historical) > query+command > query > command > anchoring.
    fn summarize_single(
        &self,
        text: &str,
        budget: usize,
        command_hint: Option<&str>,
        query: Option<&str>,
        historical_centroid: Option<&[f32]>,
    ) -> String {
        match (query, command_hint, historical_centroid) {
            // query always wins when present — user intent overrides history
            (Some(q), Some(cmd), _) if !q.is_empty() => {
                summarize_with_intent(text, budget, cmd, q).output
            }
            (Some(q), _, _) if !q.is_empty() => {
                summarize_with_query(text, budget, q).output
            }
            // historical centroid: score against what this command usually produces
            (None, Some(_), Some(centroid)) => {
                summarize_against_centroid(text, budget, centroid).output
            }
            (_, Some(_), _) => {
                summarize_with_clustering(text, budget).output
            }
            _ => {
                summarize_with_anchoring(text, budget, 1).output
            }
        }
    }

    /// Summarize a very large input by splitting into chunks of `CHUNK_SIZE_LINES`
    /// lines, summarizing each independently, then concatenating the results.
    /// Reduces peak memory compared to processing all lines at once.
    fn summarize_chunked(
        &self,
        text: &str,
        budget_per_chunk: usize,
        command_hint: Option<&str>,
        query: Option<&str>,
        historical_centroid: Option<&[f32]>,
    ) -> String {
        let lines: Vec<&str> = text.lines().collect();
        let mut parts: Vec<String> = Vec::new();

        for chunk in lines.chunks(CHUNK_SIZE_LINES) {
            let chunk_text = chunk.join("\n");
            if chunk_text.trim().is_empty() {
                continue;
            }
            let summary = self.summarize_single(&chunk_text, budget_per_chunk, command_hint, query, historical_centroid);
            if !summary.trim().is_empty() {
                parts.push(summary);
            }
        }

        if parts.len() <= 1 {
            return parts.into_iter().next().unwrap_or_default();
        }

        // Join chunk summaries with a separator so the reader knows output was chunked
        parts.join(&format!("\n[--- {} more lines ---]\n", CHUNK_SIZE_LINES))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CcrConfig, CommandConfig, FilterAction, FilterPattern, SimpleAction};
    use std::collections::HashMap;

    fn default_pipeline() -> Pipeline {
        Pipeline::new(CcrConfig::default())
    }

    #[test]
    fn pipeline_strips_ansi_then_deduplicates() {
        let pipeline = default_pipeline();
        let input = "\x1b[32mgreen\x1b[0m\n\x1b[32mgreen\x1b[0m";
        let result = pipeline.process(input, None, None, None).unwrap();
        assert_eq!(result.output.trim(), "green");
    }

    #[test]
    fn command_hint_selects_correct_patterns() {
        let mut commands = HashMap::new();
        commands.insert(
            "cargo".to_string(),
            CommandConfig {
                patterns: vec![FilterPattern {
                    regex: "^   Compiling \\S+ v[\\d.]+".to_string(),
                    action: FilterAction::Simple(SimpleAction::Collapse),
                }],
            },
        );
        let config = CcrConfig { commands, ..CcrConfig::default() };
        let pipeline = Pipeline::new(config);
        let input = "   Compiling foo v1.0\n   Compiling bar v1.0\nerror[E0001]: bad";
        let result = pipeline.process(input, Some("cargo"), None, None).unwrap();
        assert!(result.output.contains("collapsed") || result.output.contains("Compiling"));
        assert!(result.output.contains("error[E0001]"));
    }

    #[test]
    fn no_command_hint_uses_global_rules_only() {
        let mut commands = HashMap::new();
        commands.insert(
            "cargo".to_string(),
            CommandConfig {
                patterns: vec![FilterPattern {
                    regex: "^   Compiling \\S+ v[\\d.]+".to_string(),
                    action: FilterAction::Simple(SimpleAction::Remove),
                }],
            },
        );
        let config = CcrConfig { commands, ..CcrConfig::default() };
        let pipeline = Pipeline::new(config);
        let input = "   Compiling foo v1.0\n   Compiling bar v1.0";
        let result = pipeline.process(input, None, None, None).unwrap();
        assert!(result.output.contains("Compiling"));
    }

    #[test]
    fn returns_correct_analytics() {
        let pipeline = default_pipeline();
        let input = "hello world";
        let result = pipeline.process(input, None, None, None).unwrap();
        assert!(result.analytics.input_tokens > 0);
        assert!(result.analytics.output_tokens > 0);
        assert!(result.analytics.savings_pct >= 0.0);
    }
}
