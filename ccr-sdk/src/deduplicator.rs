use std::collections::HashMap;

use ccr_core::sentence::split_sentences;
use ccr_core::summarizer::embed_batch;

use crate::message::Message;

/// Similarity threshold above which a sentence is considered a duplicate of an
/// earlier sentence in the conversation.
const DEDUP_THRESHOLD: f32 = 0.92;

/// Remove redundant sentences from user messages that restate content already
/// present in an earlier turn.
///
/// For each user message (oldest to newest), sentences that are semantically
/// near-identical (cosine similarity ≥ 0.92) to a sentence in a prior user turn
/// are replaced with `[covered in turn N]`.
///
/// Assistant messages are never modified.
pub fn deduplicate(messages: Vec<Message>) -> Vec<Message> {
    // Collect all user sentences with their message index and sentence index.
    let user_sentence_positions: Vec<(usize, usize, String)> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == "user")
        .flat_map(|(msg_i, m)| {
            split_sentences(&m.content)
                .into_iter()
                .enumerate()
                .map(move |(s_i, s)| (msg_i, s_i, s))
        })
        .collect();

    // Need at least two sentences across at least two messages to dedup anything.
    let unique_msg_indices: std::collections::HashSet<usize> =
        user_sentence_positions.iter().map(|(i, _, _)| *i).collect();
    if unique_msg_indices.len() < 2 {
        return messages;
    }

    // Embed all user sentences in one batch for efficiency.
    let texts: Vec<&str> = user_sentence_positions.iter().map(|(_, _, s)| s.as_str()).collect();
    let embeddings = match embed_batch(&texts) {
        Ok(e) => e,
        Err(_) => return messages, // fall back to no-op on embedding failure
    };

    // Build a lookup: flat index → (msg_idx, sentence_idx).
    // For each sentence, find if any earlier message contains a near-duplicate.
    // replacements maps (msg_idx, sentence_idx) → the 1-based user turn number
    // of the earlier message that covers this sentence.
    let mut replacements: HashMap<(usize, usize), usize> = HashMap::new();

    // Map message index → 1-based user turn number (for display).
    let mut user_turn_number: HashMap<usize, usize> = HashMap::new();
    {
        let mut turn = 0usize;
        for (msg_i, _, _) in &user_sentence_positions {
            user_turn_number.entry(*msg_i).or_insert_with(|| {
                turn += 1;
                turn
            });
        }
    }

    for (flat_i, (msg_i, s_i, _)) in user_sentence_positions.iter().enumerate() {
        // Compare against all sentences from strictly older messages.
        for (flat_j, (prev_msg_i, _, _)) in user_sentence_positions[..flat_i].iter().enumerate() {
            if *prev_msg_i >= *msg_i {
                continue;
            }
            let sim = cosine_similarity(&embeddings[flat_i], &embeddings[flat_j]);
            if sim >= DEDUP_THRESHOLD {
                let turn_n = *user_turn_number.get(prev_msg_i).unwrap_or(&1);
                replacements.insert((*msg_i, *s_i), turn_n);
                break;
            }
        }
    }

    if replacements.is_empty() {
        return messages;
    }

    messages
        .into_iter()
        .enumerate()
        .map(|(msg_i, mut msg)| {
            if msg.role != "user" {
                return msg;
            }
            let sentences = split_sentences(&msg.content);
            let new_parts: Vec<String> = sentences
                .into_iter()
                .enumerate()
                .map(|(s_i, s)| {
                    if let Some(&turn_n) = replacements.get(&(msg_i, s_i)) {
                        format!("[covered in turn {}]", turn_n)
                    } else {
                        s
                    }
                })
                .collect();
            msg.content = new_parts.join(" ");
            msg
        })
        .collect()
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    // embed_batch returns L2-normalized vectors, so cosine similarity = dot product.
    // Clamped to [-1, 1] to absorb floating-point rounding near unit length.
    let v: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    v.clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, content: &str) -> Message {
        Message { role: role.to_string(), content: content.to_string() }
    }

    #[test]
    fn passthrough_with_single_user_message() {
        let msgs = vec![
            msg("user", "Hello. How are you?"),
            msg("assistant", "Fine."),
        ];
        let result = deduplicate(msgs.clone());
        assert_eq!(result[0].content, msgs[0].content);
    }

    #[test]
    fn assistant_messages_never_modified() {
        let msgs = vec![
            msg("assistant", "Remember: always use Rust."),
            msg("user", "Ok."),
            msg("assistant", "Remember: always use Rust."),
        ];
        let result = deduplicate(msgs.clone());
        assert_eq!(result[0].content, msgs[0].content);
        assert_eq!(result[2].content, msgs[2].content);
    }
}
