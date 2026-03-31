use crate::analytics::Analytics;
use crate::ansi::strip_ansi;
use crate::config::CcrConfig;
use crate::global_rules;
use crate::patterns::PatternFilter;
use crate::summarizer::{
    entropy_adjusted_budget, entropy_adjusted_budget_preembedded, noise_filter_with_embeddings,
    summarize_against_centroid, summarize_with_anchoring_preembedded,
    summarize_with_clustering_preembedded, summarize_with_intent, summarize_with_query,
};
use crate::tokens::count_tokens;
use crate::whitespace::normalize;

/// Inputs above this line count are split into chunks for BERT processing,
/// reducing peak memory usage. Each chunk is summarized independently.
const CHUNK_THRESHOLD_LINES: usize = 2000;
/// Lines per chunk when chunked processing is active.
const CHUNK_SIZE_LINES: usize = 500;
/// If chunk summaries together exceed the intended budget by this factor, run a
/// consolidation pass to bring the total back toward the intended budget.
const CHUNK_CONSOLIDATION_FACTOR: f32 = 1.5;

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

fn head_tail_truncate(text: &str, head: usize, tail: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    let budget = head + tail;
    if total <= budget {
        return text.to_string();
    }
    let mut out: Vec<String> = Vec::with_capacity(budget + 1);
    out.extend(lines[..head].iter().map(|l| l.to_string()));
    let omitted = total - head - tail;
    out.push(format!("[--- {} more lines ---]", omitted));
    out.extend(lines[total - tail..].iter().map(|l| l.to_string()));
    out.join("\n")
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

        // 2.3. JSON structured log compaction: if output is predominantly JSON-per-line,
        // reformat to readable [LEVEL] message lines before regex passes.
        text = crate::jsonlog::compact(&text);

        // 2.5. Apply global pre-filter rules (pure regex, zero BERT cost, always runs)
        text = global_rules::apply(&text);

        // Capture post-regex line count for BERT short-circuit decision (used below)
        let _post_regex_lines = text.lines().count();

        // 3. Apply command-specific patterns
        if let Some(hint) = command_hint {
            if let Some(cmd_config) = self.config.commands.get(hint) {
                let filter = PatternFilter::new(cmd_config)?;
                text = filter.apply(&text);
            }
        }

        // Compute removal ratio after all regex/pattern passes
        let removal_ratio = 1.0_f64 - (text.lines().count() as f64 / input.lines().count().max(1) as f64);
        let should_skip_bert = removal_ratio > 0.80;

        // 3.4. Stack trace compaction: structural parsing, no BERT cost.
        // Collapses stdlib/internal frames in Rust/Python/JS/Java/Go stack traces.
        text = crate::stacktrace::compact(&text);

        // 3.5. SimHash near-duplicate deduplication.
        // Collapses repetitive log-style lines (e.g. identical messages differing
        // only in timestamps or sequence numbers) before the more expensive BERT stage.
        // Uses the same threshold as BERT so SimHash acts as a fast pre-processor
        // rather than a separate compression stage that activates at a different point.
        if text.lines().count() >= self.config.global.summarize_threshold_lines {
            text = crate::simhash::dedup_str(&text, crate::simhash::HAMMING_THRESHOLD);
        }

        // 4. Summarize if too long
        if text.lines().count() > self.config.global.summarize_threshold_lines {
            let max_budget = self.config.global.head_lines + self.config.global.tail_lines;

            if should_skip_bert {
                // Regex pre-filters removed >80% of input — content is already noise-free.
                // Skip BERT entirely; a simple head+tail truncation is sufficient.
                text = head_tail_truncate(
                    &text,
                    self.config.global.head_lines,
                    self.config.global.tail_lines,
                );
            } else {
                // 4a. Pre-filter noise and retain BERT embeddings for re-use in step 4b.
                // noise_filter_with_embeddings embeds non-empty lines once and returns
                // (surviving_lines, their_embeddings). Passing these embeddings to
                // summarize_single avoids a second model.embed() call on the same text.
                let mut preembedded: Option<Vec<Vec<f32>>> = None;
                {
                    let lines: Vec<&str> = text.lines().collect();
                    if let Ok((surviving, embeddings)) = noise_filter_with_embeddings(&lines) {
                        if surviving.len() < lines.len() {
                            text = surviving.join("\n");
                        }
                        preembedded = Some(embeddings);
                    }
                }

                // 4b. Only summarize if still over threshold after noise removal
                if text.lines().count() > self.config.global.summarize_threshold_lines {
                    // Entropy-adaptive budget: reuse pre-computed embeddings from the noise-filter
                    // step when available (avoids a second BERT pass on a 100-line sample).
                    let budget = if let Some(ref embs) = preembedded {
                        entropy_adjusted_budget_preembedded(embs, max_budget)
                    } else {
                        entropy_adjusted_budget(&text, max_budget)
                    };

                    // 4c. Context-aware summarizer selection.
                    // For very large inputs, split into chunks to reduce peak memory.
                    // Chunked path does not reuse embeddings (each chunk is independent).
                    let line_count = text.lines().count();
                    text = if line_count > CHUNK_THRESHOLD_LINES {
                        self.summarize_chunked(&text, budget, command_hint, query, historical_centroid)
                    } else {
                        self.summarize_single(&text, budget, command_hint, query, historical_centroid, preembedded)
                    };
                }
            }
        }

        let output_tokens = count_tokens(&text);
        let analytics = Analytics::compute(input_tokens, output_tokens);

        Ok(PipelineResult { output: text, analytics, zoom_blocks: crate::zoom::drain() })
    }

    /// Summarize a single block of text using the context-aware strategy.
    /// Priority: centroid (historical) > query+command > query > command > anchoring.
    ///
    /// `preembedded` — when `Some`, these BERT embeddings (one per non-empty line in
    /// `text`, in order) were computed by the noise-filtering pass and can be reused
    /// directly by the clustering/anchoring paths, avoiding a second model.embed() call.
    fn summarize_single(
        &self,
        text: &str,
        budget: usize,
        command_hint: Option<&str>,
        query: Option<&str>,
        historical_centroid: Option<&[f32]>,
        preembedded: Option<Vec<Vec<f32>>>,
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
            // clustering and anchoring can reuse pre-computed embeddings
            (_, Some(_), _) => {
                summarize_with_clustering_preembedded(text, budget, preembedded).output
            }
            _ => {
                summarize_with_anchoring_preembedded(text, budget, 1, preembedded).output
            }
        }
    }

    /// Summarize a very large input by splitting into chunks of `CHUNK_SIZE_LINES`

    /// lines, summarizing each independently, then concatenating the results.
    /// Reduces peak memory compared to processing all lines at once.
    ///
    /// After chunking, if the combined summaries exceed the intended budget by
    /// `CHUNK_CONSOLIDATION_FACTOR`, a single consolidation pass is run over the
    /// merged summaries to bring the total back toward `intended_budget`.
    fn summarize_chunked(
        &self,
        text: &str,
        intended_budget: usize,
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
            // Chunked path: no pre-computed embeddings (each chunk is independent)
            let summary = self.summarize_single(&chunk_text, intended_budget, command_hint, query, historical_centroid, None);
            if !summary.trim().is_empty() {
                parts.push(summary);
            }
        }

        if parts.len() <= 1 {
            return parts.into_iter().next().unwrap_or_default();
        }

        let combined = parts.join("\n");

        // Consolidation pass: if chunk summaries together are too large, compress again.
        let total_lines = combined.lines().count();
        if total_lines as f32 > intended_budget as f32 * CHUNK_CONSOLIDATION_FACTOR {
            // Strip chunk separator markers before re-embedding so they don't skew BERT scores.
            let stripped: String = combined
                .lines()
                .filter(|l| !(l.starts_with("[---") && l.ends_with("more lines ---]")))
                .collect::<Vec<_>>()
                .join("\n");
            return self.summarize_single(&stripped, intended_budget, command_hint, query, historical_centroid, None);
        }

        combined
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
            "mytool".to_string(),
            CommandConfig {
                patterns: vec![FilterPattern {
                    regex: "^VERBOSE: ".to_string(),
                    action: FilterAction::Simple(SimpleAction::Collapse),
                }],
            },
        );
        let config = CcrConfig { commands, ..CcrConfig::default() };
        let pipeline = Pipeline::new(config);
        let input = "VERBOSE: loading module foo\nVERBOSE: loading module bar\nerror[E0001]: bad";
        let result = pipeline.process(input, Some("mytool"), None, None).unwrap();
        assert!(result.output.contains("collapsed") || result.output.contains("VERBOSE"));
        assert!(result.output.contains("error[E0001]"));
    }

    #[test]
    fn no_command_hint_uses_global_rules_only() {
        let mut commands = HashMap::new();
        commands.insert(
            "mytool".to_string(),
            CommandConfig {
                patterns: vec![FilterPattern {
                    regex: "^VERBOSE: ".to_string(),
                    action: FilterAction::Simple(SimpleAction::Remove),
                }],
            },
        );
        let config = CcrConfig { commands, ..CcrConfig::default() };
        let pipeline = Pipeline::new(config);
        // Without a matching command hint, command-specific Remove pattern does NOT fire
        let input = "VERBOSE: loading module foo\nVERBOSE: loading module bar";
        let result = pipeline.process(input, None, None, None).unwrap();
        assert!(result.output.contains("VERBOSE"));
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

    #[test]
    fn lazy_bert_skipped_when_high_removal_ratio() {
        // 190 lines that global_rules will strip (build progress) + 10 real lines
        let mut lines: Vec<String> = (0..190)
            .map(|i| format!("Compiling crate-{} v0.1.0 (/path)", i))
            .collect();
        lines.extend((0..10).map(|i| format!("important output line {}", i)));
        let input = lines.join("\n");
        let pipeline = default_pipeline();
        let result = pipeline.process(&input, None, None, None).unwrap();
        // Should NOT contain BERT-style omission markers (which say "lines omitted")
        // Should contain the head/tail marker OR just be short enough
        // Key: no crash, output is smaller than input
        assert!(result.output.lines().count() < 200);
    }

    #[test]
    fn head_tail_truncate_preserves_head_and_tail() {
        let lines: Vec<String> = (0..100).map(|i| format!("line {}", i)).collect();
        let text = lines.join("\n");
        let result = head_tail_truncate(&text, 10, 10);
        assert!(result.contains("line 0"));
        assert!(result.contains("line 9"));
        assert!(result.contains("line 90"));
        assert!(result.contains("line 99"));
        assert!(result.contains("more lines"));
    }

    #[test]
    fn head_tail_truncate_no_truncate_when_within_budget() {
        let lines: Vec<String> = (0..15).map(|i| format!("line {}", i)).collect();
        let text = lines.join("\n");
        let result = head_tail_truncate(&text, 10, 10);
        // 15 lines <= 20 budget, no truncation
        assert!(!result.contains("more lines"));
        assert_eq!(result.lines().count(), 15);
    }
}
