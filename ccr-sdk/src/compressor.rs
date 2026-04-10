use panda_core::summarizer::{semantic_similarity, summarize_assistant_message, summarize_message};
use panda_core::tokens::count_tokens;

use crate::message::Message;
use crate::ollama::OllamaConfig;

/// Controls how aggressively each tier of the conversation is compressed.
///
/// Turn age is counted from the most recent message (age 0 = latest).
///
/// ```text
/// age:  0        recent_n    recent_n+tier1_n    ...
///       |──────────|────────────|─────────────────|
///       keep verbatim  tier 1       tier 2
/// ```
///
/// Tier 2 uses Ollama for generative summarization when `ollama` is set and
/// the service is reachable. BERT similarity gates the output — if the generated
/// summary drifts below `similarity_threshold`, we fall back to extractive.
pub struct CompressionConfig {
    /// Most-recent N turns kept verbatim.
    pub recent_n: usize,
    /// Next N turns after recent_n get light extractive compression.
    pub tier1_n: usize,
    /// Sentence budget ratio for tier 1 — keeps ~55% of sentences.
    pub tier1_ratio: f32,
    /// Sentence budget ratio for tier 2 extractive fallback — keeps ~20% of sentences.
    pub tier2_ratio: f32,
    /// Sentence budget ratio for assistant messages in tier 2 — keeps ~60% of sentences.
    /// Set to 1.0 to keep assistant messages verbatim (old behaviour).
    pub tier2_assistant_ratio: f32,
    /// When set, tier 2 uses Ollama generative summarization with BERT quality gate.
    pub ollama: Option<OllamaConfig>,
    /// When set, the compressor dynamically compresses enough history to stay under
    /// this token budget. Applied as a second pass after tier-based compression.
    pub max_context_tokens: Option<usize>,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            recent_n: 3,
            tier1_n: 5,
            tier1_ratio: 0.55,
            tier2_ratio: 0.20,
            tier2_assistant_ratio: 0.60,
            ollama: None,
            max_context_tokens: None,
        }
    }
}

pub struct CompressResult {
    pub messages: Vec<Message>,
    /// Total tokens across all messages before compression.
    pub tokens_in: usize,
    /// Total tokens across all messages after compression.
    pub tokens_out: usize,
}

// Per-message compression state used during the budget pass.
struct MsgState {
    role: String,
    original: String,
    current: String,
    /// 0 = verbatim, 1 = tier1 ratio, 2 = tier2 / max compression
    tier: u8,
}

/// Compress a conversation history using tiered semantic compression.
///
/// Rules:
/// - User messages are compressed based on their age in the conversation.
/// - Assistant messages are kept verbatim in recent/tier1 windows; compressed at tier2.
/// - Tier 2 with Ollama: generative summarization → BERT similarity check → extractive fallback.
/// - If `max_context_tokens` is set, a second budget pass compresses further until under limit.
pub fn compress(messages: Vec<Message>, config: &CompressionConfig) -> CompressResult {
    let total = messages.len();
    let tokens_in: usize = messages.iter().map(|m| count_tokens(&m.content)).sum();

    // Check Ollama availability once upfront — avoids per-message HTTP pings.
    let ollama_available = config
        .ollama
        .as_ref()
        .map(|o| crate::ollama::is_available(o))
        .unwrap_or(false);

    let mut states: Vec<MsgState> = messages
        .into_iter()
        .enumerate()
        .map(|(i, msg)| {
            let age = total - 1 - i;
            let is_tier2 = age >= config.recent_n + config.tier1_n;
            let is_tier1 = age >= config.recent_n && !is_tier2;

            let (current, tier) = if msg.role != "user" {
                // Assistant messages: verbatim in recent/tier1, light compression in tier2.
                if is_tier2 && config.tier2_assistant_ratio < 1.0 {
                    let c = summarize_assistant_message(&msg.content, config.tier2_assistant_ratio).output;
                    (c, 2u8)
                } else {
                    (msg.content.clone(), 0u8)
                }
            } else if age < config.recent_n {
                (msg.content.clone(), 0u8)
            } else if is_tier1 {
                (summarize_message(&msg.content, config.tier1_ratio).output, 1u8)
            } else {
                // tier2 user message
                let c = if ollama_available {
                    compress_tier2_generative(&msg.content, config)
                } else {
                    summarize_message(&msg.content, config.tier2_ratio).output
                };
                (c, 2u8)
            };

            MsgState { role: msg.role, original: msg.content, current, tier }
        })
        .collect();

    // Budget pass: if total tokens still exceed the limit, compress further starting
    // from the oldest messages.
    if let Some(max_tokens) = config.max_context_tokens {
        apply_budget_pass(&mut states, max_tokens, config);
    }

    let compressed: Vec<Message> = states
        .into_iter()
        .map(|s| Message { role: s.role, content: s.current })
        .collect();

    let tokens_out: usize = compressed.iter().map(|m| count_tokens(&m.content)).sum();

    CompressResult { messages: compressed, tokens_in, tokens_out }
}

/// Second-pass budget enforcement. Walks from oldest to newest, pushing messages
/// to higher compression tiers until total tokens fall under `max_tokens`.
fn apply_budget_pass(states: &mut Vec<MsgState>, max_tokens: usize, config: &CompressionConfig) {
    let n = states.len();
    let mut total_tokens: usize = states.iter().map(|s| count_tokens(&s.current)).sum();
    if total_tokens <= max_tokens {
        return;
    }

    // Pass 1: push user messages to higher tiers (oldest first).
    for i in 0..n {
        if total_tokens <= max_tokens {
            return;
        }
        let age = n - 1 - i;
        let s = &mut states[i];
        if s.role != "user" {
            continue;
        }
        let new_opt: Option<(String, u8)> = match s.tier {
            0 if age >= config.recent_n => {
                Some((summarize_message(&s.original, config.tier1_ratio).output, 1))
            }
            1 => Some((summarize_message(&s.original, config.tier2_ratio).output, 2)),
            _ => None,
        };
        if let Some((new_content, new_tier)) = new_opt {
            let old_tok = count_tokens(&s.current);
            let new_tok = count_tokens(&new_content);
            total_tokens = total_tokens.saturating_sub(old_tok) + new_tok;
            s.current = new_content;
            s.tier = new_tier;
        }
    }

    // Pass 2: compress non-recent assistant messages if still over budget.
    if total_tokens > max_tokens && config.tier2_assistant_ratio < 1.0 {
        for i in 0..n {
            if total_tokens <= max_tokens {
                return;
            }
            let age = n - 1 - i;
            let s = &mut states[i];
            if s.role == "user" || s.tier >= 2 || age < config.recent_n {
                continue;
            }
            let new_content = summarize_assistant_message(&s.original, config.tier2_assistant_ratio).output;
            let old_tok = count_tokens(&s.current);
            let new_tok = count_tokens(&new_content);
            total_tokens = total_tokens.saturating_sub(old_tok) + new_tok;
            s.current = new_content;
            s.tier = 2;
        }
    }
}

/// Tier 2 generative path: Ollama summarizes → BERT gates → extractive fallback.
fn compress_tier2_generative(text: &str, config: &CompressionConfig) -> String {
    let ollama = config.ollama.as_ref().expect("checked before calling");

    match crate::ollama::summarize(text, ollama) {
        Ok(generated) => {
            // BERT quality gate: reject if generated output drifts semantically.
            let similarity = semantic_similarity(text, &generated).unwrap_or(0.0);
            if similarity >= ollama.similarity_threshold {
                generated
            } else {
                // Generative output drifted — fall back to extractive.
                summarize_message(text, config.tier2_ratio).output
            }
        }
        // Ollama call failed mid-run — fall back silently.
        Err(_) => summarize_message(text, config.tier2_ratio).output,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_messages(pairs: &[(&str, &str)]) -> Vec<Message> {
        pairs
            .iter()
            .map(|(role, content)| Message {
                role: role.to_string(),
                content: content.to_string(),
            })
            .collect()
    }

    #[test]
    fn recent_messages_kept_verbatim() {
        let msgs = make_messages(&[
            ("user", "Old message that should be compressed because it has many sentences. It goes on and on. There is a lot of filler here. Make sure this is handled."),
            ("assistant", "Response."),
            ("user", "Recent user message."),
            ("assistant", "Recent assistant response."),
            ("user", "Most recent message."),
        ]);
        let config = CompressionConfig { recent_n: 2, ..Default::default() };
        let result = compress(msgs, &config);
        assert_eq!(result.messages[4].content, "Most recent message.");
    }

    #[test]
    fn assistant_messages_verbatim_when_ratio_is_one() {
        let long_assistant = "Assistant said this. It is very detailed. There are many sentences here. Each one matters. Do not compress this ever.";
        let msgs = make_messages(&[
            ("assistant", long_assistant),
            ("user", "user"),
            ("user", "user"),
            ("user", "user"),
            ("user", "user"),
        ]);
        // tier2_assistant_ratio: 1.0 → verbatim even in tier 2
        let config = CompressionConfig {
            recent_n: 0, tier1_n: 1, tier1_ratio: 0.3, tier2_ratio: 0.1,
            tier2_assistant_ratio: 1.0, ollama: None, max_context_tokens: None,
        };
        let result = compress(msgs, &config);
        assert_eq!(result.messages[0].content, long_assistant);
    }

    #[test]
    fn assistant_messages_compressed_in_tier2() {
        let long_assistant = "The project is building a token reducer. We use BERT embeddings for scoring. The config must be in TOML format. Errors are never dropped. We target 60% sentence retention in tier 2. Performance requirements are strict.";
        let msgs = make_messages(&[
            ("assistant", long_assistant),
            ("user", "user turn 1"),
            ("user", "user turn 2"),
            ("user", "user turn 3"),
            ("user", "user turn 4"),
            ("user", "user turn 5"),
            ("user", "user turn 6"),
        ]);
        // With recent_n=3, tier1_n=2, the assistant at index 0 has age=6 → tier2
        let config = CompressionConfig {
            recent_n: 3, tier1_n: 2, tier1_ratio: 0.55, tier2_ratio: 0.20,
            tier2_assistant_ratio: 0.60, ollama: None, max_context_tokens: None,
        };
        let result = compress(msgs, &config);
        // Should be compressed (shorter than original)
        assert!(result.messages[0].content.len() < long_assistant.len());
    }

    #[test]
    fn budget_pass_reduces_tokens_to_target() {
        let long_msg = "This is a sentence about the project. We are building a token reducer. It should work well. Make sure errors are never dropped. The config should be in TOML format. We want fast performance.";
        let mut pairs: Vec<(&str, &str)> = Vec::new();
        for _ in 0..8 {
            pairs.push(("user", long_msg));
            pairs.push(("assistant", "Ok."));
        }
        let msgs = make_messages(&pairs);
        let total_tokens: usize = msgs.iter().map(|m| count_tokens(&m.content)).sum();
        let budget = total_tokens / 2;
        let config = CompressionConfig {
            max_context_tokens: Some(budget),
            ..Default::default()
        };
        let result = compress(msgs, &config);
        assert!(result.tokens_out <= budget);
    }

    #[test]
    fn tokens_out_less_than_tokens_in_for_long_history() {
        let long_msg = "This is a sentence about the project. We are building a token reducer. It should work well. Make sure errors are never dropped. The config should be in TOML format. We want fast performance.";
        let mut pairs: Vec<(&str, &str)> = Vec::new();
        for _ in 0..10 {
            pairs.push(("user", long_msg));
            pairs.push(("assistant", "Ok."));
        }
        let msgs = make_messages(&pairs);
        let config = CompressionConfig::default();
        let result = compress(msgs, &config);
        assert!(result.tokens_out <= result.tokens_in);
    }
}
