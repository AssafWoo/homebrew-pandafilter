use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::{self, Read};

#[derive(Debug, Deserialize)]
struct HookInput {
    #[serde(default)]
    #[allow(dead_code)]
    tool_name: String,
    #[serde(default)]
    tool_input: serde_json::Value,
    #[serde(default)]
    tool_response: ToolResponse,
}

#[derive(Debug, Deserialize, Default)]
struct ToolResponse {
    #[serde(default)]
    output: String,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct HookOutput {
    output: String,
}

pub fn run() -> Result<()> {
    let mut input = String::new();
    if io::stdin().read_to_string(&mut input).is_err() {
        return Ok(());
    }

    let hook_input: HookInput = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };

    let command_hint = hook_input
        .tool_input
        .get("command")
        .and_then(|v| v.as_str())
        .and_then(|cmd| cmd.split_whitespace().next())
        .map(|s| s.to_string());

    let output_text = if let Some(err) = &hook_input.tool_response.error {
        err.clone()
    } else {
        hook_input.tool_response.output.clone()
    };

    if output_text.is_empty() {
        return Ok(());
    }

    let config = match crate::config_loader::load_config() {
        Ok(c) => c,
        Err(_) => return Ok(()),
    };

    // B2: use command string as query for biased BERT summarization
    let query = hook_input
        .tool_input
        .get("command")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Load session BEFORE pipeline call so we can pass the historical centroid
    // and context pressure to the summarizer.
    let sid = crate::session::session_id();
    let mut session = crate::session::SessionState::load(&sid);

    // Use the first two words of the full command as the delta/centroid key.
    // "git status" is distinct from "git log"; "git status" matches "git status --short".
    let full_cmd = hook_input
        .tool_input
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let cmd_key: String = full_cmd
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");

    let historical_centroid = session.command_centroid(&cmd_key).cloned();

    // EC: tighten pipeline config proportionally to how full the context window is.
    let pressure = session.context_pressure();
    // ZI: enable Zoom-In so collapsed/omitted markers include expand IDs.
    ccr_core::zoom::enable();

    let pipeline = ccr_core::pipeline::Pipeline::new(config.with_pressure(pressure));
    let result = match pipeline.process(
        &output_text,
        command_hint.as_deref(),
        query.as_deref(),
        historical_centroid.as_deref(),
    ) {
        Ok(r) => r,
        Err(_) => return Ok(()),
    };

    // Persist zoom blocks so `ccr expand ZI_N` can retrieve them.
    let _ = crate::zoom_store::save_blocks(&sid, result.zoom_blocks);

    // ── Session-aware passes ──────────────────────────────────────────────────

    // Idea 3: Delta compression — embed pipeline output and suppress lines
    // already seen in a prior run of the same command.
    let pipeline_emb = ccr_core::summarizer::embed_batch(&[result.output.as_str()])
        .ok()
        .and_then(|mut v| v.pop());

    let output_after_delta = if let Some(ref emb) = pipeline_emb {
        let lines: Vec<&str> = result.output.lines().collect();
        session
            .compute_delta(&cmd_key, &lines, emb)
            .map(|d| d.output)
            .unwrap_or_else(|| result.output.clone())
    } else {
        result.output.clone()
    };

    // C1: Sentence-level deduplication against recent session content.
    // Marks sentences that repeat earlier tool outputs as [covered in turn N].
    let after_dedup = apply_sentence_dedup(&output_after_delta, &cmd_key, &session);

    // C2: Apply extra line compression when the session is token-heavy.
    // Idea 7: Use historical command centroid when available for smarter second-pass.
    let compression_factor = session.compression_factor();
    let centroid_for_c2 = session.command_centroid(&cmd_key).cloned();
    let mut final_output = if compression_factor < 0.90 {
        let line_count = after_dedup.lines().count();
        let reduced_budget = ((line_count as f32 * compression_factor) as usize).max(10);
        if let Some(ref centroid) = centroid_for_c2 {
            ccr_core::summarizer::summarize_against_centroid(&after_dedup, reduced_budget, centroid)
                .output
        } else {
            ccr_core::summarizer::summarize(&after_dedup, reduced_budget).output
        }
    } else {
        after_dedup
    };

    // B3: Record in session cache for cross-turn dedup on future calls.
    // Idea 7: Also update per-command historical centroid using line-mean embedding
    //         (better quality than embedding the whole output as a single string).
    if let Ok(mut embeddings) = ccr_core::summarizer::embed_batch(&[final_output.as_str()]) {
        if let Some(emb) = embeddings.pop() {
            let tokens = ccr_core::tokens::count_tokens(&final_output);
            // Update centroid with line-mean for better per-command history
            if let Ok(line_centroid) = ccr_core::summarizer::compute_output_centroid(&final_output) {
                session.update_command_centroid(&cmd_key, line_centroid);
            } else {
                session.update_command_centroid(&cmd_key, emb.clone());
            }
            // SD: state commands get full-content storage for accurate delta beyond 4000 chars.
            let is_state = {
                if let Ok(cfg) = crate::config_loader::load_config() {
                    cfg.global.state_commands.iter().any(|s| {
                        command_hint.as_deref() == Some(s.as_str())
                    })
                } else {
                    false
                }
            };
            session.record(&cmd_key, emb, tokens, &final_output, is_state);
            session.save(&sid);
        }
    }

    // EC: In critical pressure (>0.8), append a notice so the user knows why
    // output is heavily compressed.
    if pressure > 0.80 {
        final_output.push_str(
            "\n[⚠ context near full — output compressed aggressively; run `ccr gain` to review]",
        );
    }

    let hook_output = HookOutput { output: final_output };
    println!("{}", serde_json::to_string(&hook_output)?);

    Ok(())
}

/// C1: Build a deduplication context from recent session entries and apply
/// the ccr-sdk sentence deduplicator to the current output.
fn apply_sentence_dedup(
    output: &str,
    _cmd: &str,
    session: &crate::session::SessionState,
) -> String {
    use ccr_sdk::deduplicator::deduplicate;
    use ccr_sdk::message::Message;

    // Use last 8 entries as prior context regardless of command —
    // repeated file content and error messages appear across different commands.
    let prior = session.recent_content(8);
    if prior.is_empty() {
        return output.to_string();
    }

    let mut messages: Vec<Message> = prior
        .into_iter()
        .map(|(_, content)| Message { role: "user".to_string(), content })
        .collect();

    messages.push(Message {
        role: "user".to_string(),
        content: output.to_string(),
    });

    let deduped = deduplicate(messages);

    deduped
        .into_iter()
        .last()
        .map(|m| m.content)
        .unwrap_or_else(|| output.to_string())
}
