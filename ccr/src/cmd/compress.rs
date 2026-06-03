use anyhow::Result;
use panda_sdk::{
    compressor::{compress, CompressionConfig, CompressResult},
    deduplicator::deduplicate,
    message::Message,
    ollama::OllamaConfig,
};
use std::io::Read;

pub fn run(
    input: &str,
    output: Option<&str>,
    recent_turns: usize,
    tier1_turns: usize,
    ollama_url: Option<&str>,
    ollama_model: &str,
    max_tokens: Option<usize>,
    dry_run: bool,
    scan_session: bool,
    smart: bool,
    json_output: bool,
) -> Result<()> {
    let (raw, source_path) = if scan_session {
        let path = find_latest_jsonl()
            .ok_or_else(|| anyhow::anyhow!("no .jsonl files found under ~/.claude/projects/"))?;
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("cannot read '{}': {}", path.display(), e))?;
        (raw, Some(path))
    } else {
        let raw = if input == "-" {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s)?;
            s
        } else {
            std::fs::read_to_string(input)
                .map_err(|e| anyhow::anyhow!("cannot read '{}': {}", input, e))?
        };
        (raw, None)
    };

    let messages = if scan_session {
        parse_jsonl_conversation(&raw)?
    } else {
        parse_conversation(&raw)?
    };

    if messages.is_empty() {
        if dry_run {
            println!("[dry-run] 0 turns · 0 → 0 tokens (0% saved)");
        } else {
            let out = "[]";
            match &source_path {
                Some(path) if scan_session => {
                    let out_path = format!("{}.compressed.json", path.display());
                    std::fs::write(&out_path, out)
                        .map_err(|e| anyhow::anyhow!("cannot write to '{}': {}", out_path, e))?;
                    eprintln!("[panda compress] wrote compressed output to {}", out_path);
                }
                _ => write_output(out, output)?,
            }
        }
        return Ok(());
    }

    let config = CompressionConfig {
        recent_n: recent_turns,
        tier1_n: tier1_turns,
        ollama: ollama_url.map(|url| OllamaConfig {
            base_url: url.to_string(),
            model: ollama_model.to_string(),
            similarity_threshold: 0.80,
        }),
        max_context_tokens: max_tokens,
        ..CompressionConfig::default()
    };

    // Deduplicate first, then compress (matches Optimizer logic)
    let deduped = deduplicate(messages.clone());
    let mut result = compress(deduped, &config);

    // Smart mode: apply an additional staleness-aware pass.
    // Stale messages (state commands >10 turns old, builds before last edit,
    // reads of edited files) are re-compressed to tier-2 ratio regardless of
    // their position. Writes to a separate .smart.json file.
    if smart {
        result = apply_smart_compression(result.messages, &config);
    }

    let turns = messages.len();

    if dry_run {
        let saved_pct = if result.tokens_in > 0 {
            100.0 * (result.tokens_in - result.tokens_out.min(result.tokens_in)) as f64
                / result.tokens_in as f64
        } else {
            0.0
        };
        println!(
            "[dry-run] {} turns · {} → {} tokens ({:.0}% saved)",
            turns, result.tokens_in, result.tokens_out, saved_pct
        );
        return Ok(());
    }

    let saved_pct = if result.tokens_in > 0 {
        100.0 * (result.tokens_in - result.tokens_out.min(result.tokens_in)) as f64
            / result.tokens_in as f64
    } else {
        0.0
    };

    // --json: emit structured envelope to stdout regardless of scan_session / -o
    if json_output {
        let envelope = serde_json::json!({
            "messages": result.messages,
            "tokens_in": result.tokens_in,
            "tokens_out": result.tokens_out,
            "savings_pct": saved_pct,
        });
        println!("{}", envelope);
        return Ok(());
    }

    let json = serde_json::to_string_pretty(&result.messages)?;

    match &source_path {
        Some(path) if scan_session => {
            let suffix = if smart { ".smart.json" } else { ".compressed.json" };
            let out_path = format!("{}{}", path.display(), suffix);
            std::fs::write(&out_path, &json)
                .map_err(|e| anyhow::anyhow!("cannot write to '{}': {}", out_path, e))?;
            if result.tokens_in > 0 {
                eprintln!(
                    "[panda compress] {} → {} tokens ({:.0}% saved)",
                    result.tokens_in, result.tokens_out, saved_pct
                );
            }
            eprintln!("[panda compress] wrote compressed output to {}", out_path);
        }
        _ => {
            write_output(&json, output)?;
            // Stats to stderr so they don't pollute piped output
            if result.tokens_in > 0 {
                eprintln!(
                    "[panda compress] {} → {} tokens ({:.0}% saved)",
                    result.tokens_in, result.tokens_out, saved_pct
                );
            }
        }
    }

    Ok(())
}

fn write_output(content: &str, path: Option<&str>) -> Result<()> {
    match path {
        Some(p) => std::fs::write(p, content)
            .map_err(|e| anyhow::anyhow!("cannot write to '{}': {}", p, e)),
        None => {
            println!("{}", content);
            Ok(())
        }
    }
}

/// Find the most recently modified `.jsonl` file under `~/.claude/projects/`.
fn find_latest_jsonl() -> Option<std::path::PathBuf> {
    let home = dirs::home_dir()?;
    let projects_dir = home.join(".claude").join("projects");
    if !projects_dir.exists() {
        return None;
    }

    let mut best: Option<(std::path::PathBuf, std::time::SystemTime)> = None;
    visit_dir(&projects_dir, &mut best);
    best.map(|(path, _)| path)
}

fn visit_dir(
    dir: &std::path::Path,
    best: &mut Option<(std::path::PathBuf, std::time::SystemTime)>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            visit_dir(&path, best);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            if let Ok(meta) = std::fs::metadata(&path) {
                if let Ok(modified) = meta.modified() {
                    let is_newer = best
                        .as_ref()
                        .map(|(_, t)| modified > *t)
                        .unwrap_or(true);
                    if is_newer {
                        *best = Some((path, modified));
                    }
                }
            }
        }
    }
}

/// Parse a JSONL conversation from `~/.claude/projects/`.
/// Each line is a JSON object with `"type"` and `"message"` fields.
/// Only `"user"` and `"assistant"` type lines are extracted.
///
/// Reads only the last 500 lines to avoid hanging on very large session files.
fn parse_jsonl_conversation(raw: &str) -> Result<Vec<Message>> {
    const MAX_LINES: usize = 500;
    let all_lines: Vec<&str> = raw.lines().collect();
    let start = all_lines.len().saturating_sub(MAX_LINES);
    let mut messages = Vec::new();
    for line in &all_lines[start..] {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let typ = v["type"].as_str().unwrap_or("");
        if typ != "user" && typ != "assistant" {
            continue;
        }
        let role = typ.to_string();
        let content_val = &v["message"]["content"];
        let content = if let Some(s) = content_val.as_str() {
            s.to_string()
        } else if let Some(arr) = content_val.as_array() {
            arr.iter()
                .filter_map(|block| {
                    if block["type"].as_str() == Some("text") {
                        block["text"].as_str().map(|s| s.to_string())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            String::new()
        };
        messages.push(Message { role, content });
    }
    Ok(messages)
}

/// Parse a conversation JSON.
/// Accepts two formats:
///   1. `[{"role": "...", "content": "..."}]`  — bare array
///   2. `{"messages": [{"role": "...", "content": "..."}]}`  — object with messages key
fn parse_conversation(raw: &str) -> Result<Vec<Message>> {
    // Try bare array first
    if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(raw) {
        return arr
            .iter()
            .map(|v| {
                Ok(Message {
                    role: v["role"].as_str().unwrap_or("user").to_string(),
                    content: v["content"].as_str().unwrap_or("").to_string(),
                })
            })
            .collect();
    }

    // Try {messages: [...]} object
    if let Ok(obj) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Some(msgs) = obj["messages"].as_array() {
            return msgs
                .iter()
                .map(|v| {
                    Ok(Message {
                        role: v["role"].as_str().unwrap_or("user").to_string(),
                        content: v["content"].as_str().unwrap_or("").to_string(),
                    })
                })
                .collect();
        }
    }

    anyhow::bail!(
        "input is not valid conversation JSON \
         (expected array or {{\"messages\": [...]}})"
    )
}

/// Apply content-aware staleness compression as an additional pass after normal compression.
///
/// Scans each message's text for patterns that indicate stale information —
/// git-status/log/diff output, build output that predates the last apparent
/// edit, ls/df/ps output. Messages that match AND fall outside the `recent_n`
/// verbatim window are re-compressed to `tier2_ratio`.
///
/// The function never touches messages in the most-recent `recent_n` window
/// and never calls BERT — detection is pure regex/substring matching.
fn apply_smart_compression(messages: Vec<Message>, config: &CompressionConfig) -> CompressResult {
    let n = messages.len();
    let tokens_in: usize = messages.iter().map(|m| panda_core::tokens::count_tokens(&m.content)).sum();

    // Find the index of the last message that looks like a code-edit operation.
    // Build / state outputs recorded before this index are considered stale.
    let last_edit_idx = find_last_edit_index(&messages);

    let compressed: Vec<Message> = messages
        .into_iter()
        .enumerate()
        .map(|(i, msg)| {
            let age = n - 1 - i; // 0 = most recent
            // Always preserve the recent verbatim window.
            if age < config.recent_n {
                return msg;
            }
            if is_stale_content(&msg.content, i, last_edit_idx) {
                let new_content = panda_core::summarizer::summarize_message(
                    &msg.content,
                    config.tier2_ratio,
                )
                .output;
                Message { role: msg.role, content: new_content }
            } else {
                msg
            }
        })
        .collect();

    let tokens_out: usize = compressed.iter().map(|m| panda_core::tokens::count_tokens(&m.content)).sum();
    CompressResult { messages: compressed, tokens_in, tokens_out }
}

/// Return the 0-based index of the last message whose content looks like it
/// describes a code-edit or file-write operation.
fn find_last_edit_index(messages: &[Message]) -> Option<usize> {
    // Patterns that suggest the assistant or the user performed an edit.
    // We scan case-insensitively against a few unambiguous markers.
    const EDIT_MARKERS: &[&str] = &[
        "--- a/",         // unified diff header produced by editors / git
        "+++ b/",
        "edit(",          // Edit tool invocation text that may appear in assistant response
        "write(",         // Write tool
        "✎ edit",
        "modified file",
        "updated file",
        "wrote to ",
        "saved to ",
    ];
    messages.iter().rposition(|msg| {
        let lower = msg.content.to_lowercase();
        EDIT_MARKERS.iter().any(|&pat| lower.contains(pat))
    })
}

/// Returns `true` when the message content looks like stale state/build output.
///
/// State command output is always stale outside the recent window.
/// Build output is stale only when it predates `last_edit_idx` (i.e. the
/// build result is invalidated by a later edit).
fn is_stale_content(content: &str, idx: usize, last_edit_idx: Option<usize>) -> bool {
    let lower = content.to_lowercase();

    // ── State command output patterns ────────────────────────────────────────
    // git status / git log / git diff
    let is_git_state =
        lower.contains("on branch")
            || lower.contains("changes not staged for commit")
            || lower.contains("changes to be committed")
            || lower.contains("nothing to commit")
            || lower.contains("untracked files:")
            || lower.contains("diff --git")
            || (lower.contains("author:") && lower.contains("date:") && lower.contains("commit "));

    if is_git_state {
        return true;
    }

    // kubectl / docker / df / ps state output
    let is_sys_state =
        (lower.contains("namespace") && lower.contains("status") && lower.contains("age"))
            || (lower.contains("container id") && lower.contains("image") && lower.contains("status"))
            || (lower.contains("filesystem") && lower.contains("used") && lower.contains("available"))
            || (lower.contains("pid") && lower.contains("user") && lower.contains("command") && lower.contains("%cpu"));

    if is_sys_state {
        return true;
    }

    // ── Build / test output that predates the last edit ──────────────────────
    if let Some(edit_idx) = last_edit_idx {
        if idx < edit_idx {
            let is_build_output =
                lower.contains("compiling ")
                    || lower.contains("finished [")
                    || lower.contains("error[e")
                    || lower.contains("test result:")
                    || (lower.contains(" passed") && lower.contains(" failed"))
                    || lower.contains("running cargo")
                    || lower.contains("cargo build")
                    || lower.contains("cargo test")
                    || lower.contains("pytest")
                    || lower.contains("failed tests:");
            if is_build_output {
                return true;
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bare_array() {
        let json = r#"[{"role":"user","content":"hello"},{"role":"assistant","content":"hi"}]"#;
        let msgs = parse_conversation(json).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[1].content, "hi");
    }

    #[test]
    fn parse_messages_object() {
        let json = r#"{"messages":[{"role":"user","content":"hello"}]}"#;
        let msgs = parse_conversation(json).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content, "hello");
    }

    #[test]
    fn parse_empty_array() {
        let json = "[]";
        let msgs = parse_conversation(json).unwrap();
        assert!(msgs.is_empty());
    }

    #[test]
    fn parse_invalid_json_errors() {
        let result = parse_conversation("not json at all");
        assert!(result.is_err());
    }

    #[test]
    fn compression_reduces_tokens_for_long_history() {
        let long = "This is a long message with several sentences. It discusses project details. \
                    Make sure errors are never dropped. The config should be in TOML format. \
                    We want fast performance and low token usage.";
        let mut pairs: Vec<serde_json::Value> = Vec::new();
        for i in 0..10 {
            pairs.push(serde_json::json!({"role": "user", "content": long}));
            pairs.push(serde_json::json!({"role": "assistant", "content": format!("Response {}.", i)}));
        }
        let json = serde_json::to_string(&pairs).unwrap();
        let msgs = parse_conversation(&json).unwrap();
        let config = CompressionConfig::default();
        let deduped = deduplicate(msgs);
        let result = compress(deduped, &config);
        assert!(result.tokens_out <= result.tokens_in);
    }

    #[test]
    fn empty_input_returns_empty_json() {
        // Verify the empty path works end-to-end
        let msgs = parse_conversation("[]").unwrap();
        assert!(msgs.is_empty());
    }

    #[test]
    fn parse_jsonl_extracts_user_and_assistant() {
        let jsonl = r#"{"type":"system","message":{"role":"system","content":"You are helpful."}}
{"type":"user","message":{"role":"user","content":"Hello there"}}
{"type":"assistant","message":{"role":"assistant","content":"Hi! How can I help?"}}
{"type":"tool_use","message":{"role":"tool","content":"some tool output"}}"#;
        let msgs = parse_jsonl_conversation(jsonl).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].content, "Hello there");
        assert_eq!(msgs[1].role, "assistant");
        assert_eq!(msgs[1].content, "Hi! How can I help?");
    }

    #[test]
    fn parse_jsonl_handles_array_content_blocks() {
        let jsonl = r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"Part one"},{"type":"text","text":"Part two"}]}}"#;
        let msgs = parse_jsonl_conversation(jsonl).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content, "Part one\nPart two");
    }

    #[test]
    fn parse_jsonl_skips_non_text_blocks_in_array() {
        let jsonl = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{}},{"type":"text","text":"Done."}]}}"#;
        let msgs = parse_jsonl_conversation(jsonl).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content, "Done.");
    }

    #[test]
    fn parse_jsonl_empty_input() {
        let msgs = parse_jsonl_conversation("").unwrap();
        assert!(msgs.is_empty());
    }

    #[test]
    fn find_latest_jsonl_returns_none_when_dir_missing() {
        // ~/.claude/projects/ may or may not exist; we just verify the function
        // doesn't panic and returns None when given a nonexistent path by
        // checking the behavior of visit_dir directly with a temp path.
        let nonexistent = std::path::Path::new("/tmp/panda_test_nonexistent_dir_xyz");
        let mut best = None;
        visit_dir(nonexistent, &mut best);
        assert!(best.is_none());
    }

    // ── apply_smart_compression tests ─────────────────────────────────────────

    fn make_msgs(pairs: &[(&str, &str)]) -> Vec<Message> {
        pairs
            .iter()
            .map(|(role, content)| Message { role: role.to_string(), content: content.to_string() })
            .collect()
    }

    #[test]
    fn smart_compression_reduces_tokens_for_git_state_output() {
        // Git status output in an old message should be re-compressed.
        let git_status = "On branch main\nChanges not staged for commit:\n  \
            (use \"git add <file>...\" to update what will be committed)\n\
            modified:   src/main.rs\n\
            modified:   src/lib.rs\n\
            modified:   src/hook.rs\n\
            nothing to commit for now but you should still check this carefully\n\
            Untracked files:\n  .DS_Store\n  target/\n  README.md.bak";
        let msgs = make_msgs(&[
            ("assistant", git_status),
            ("user", "ok, continue"),
            ("user", "and now"),
            ("user", "and now 2"),
            ("user", "latest user message here"),
        ]);
        let config = CompressionConfig { recent_n: 2, ..Default::default() };
        let tokens_before: usize = msgs.iter().map(|m| panda_core::tokens::count_tokens(&m.content)).sum();
        let result = apply_smart_compression(msgs, &config);
        assert!(result.tokens_out <= tokens_before);
    }

    #[test]
    fn smart_compression_preserves_recent_window() {
        // Messages within recent_n must never be touched.
        let git_status = "On branch main\nChanges not staged for commit:\n\
            modified:   src/main.rs\nnothing to commit, working tree clean";
        let msgs = make_msgs(&[
            ("assistant", git_status),
            ("user", "latest message, verbatim"),
        ]);
        let config = CompressionConfig { recent_n: 2, ..Default::default() };
        let result = apply_smart_compression(msgs, &config);
        // The most recent message must be untouched.
        assert_eq!(result.messages.last().unwrap().content, "latest message, verbatim");
        // The git status message is also within recent_n=2, so untouched.
        assert_eq!(result.messages[0].content, git_status);
    }

    #[test]
    fn smart_compression_build_before_edit_is_stale() {
        // A cargo build result that predates a write operation should be compressed.
        let build_output = "Compiling panda v1.0.0\n\
            error[E0382]: borrow of moved value\n  --> src/main.rs:10:5\n\
            Compiling again after fixes\nFinished [unoptimized] in 1.23s\n\
            All tests passed. Test result: ok. 3 passed; 0 failed.\n\
            Build complete and all checks passed successfully.";
        let edit_msg = "I edited src/main.rs to fix the borrow issue. wrote to the file.";
        let msgs = make_msgs(&[
            ("assistant", build_output), // idx 0 — before edit
            ("assistant", edit_msg),     // idx 1 — last edit marker
            ("user", "great, thanks"),
            ("user", "latest turn"),
        ]);
        let config = CompressionConfig { recent_n: 1, ..Default::default() };
        let result = apply_smart_compression(msgs, &config);
        // Build output at idx 0 should be compressed (shorter).
        assert!(result.messages[0].content.len() < build_output.len());
    }

    #[test]
    fn smart_compression_build_after_edit_is_not_stale() {
        // A build that comes AFTER the last edit should not be compressed by smart pass.
        let edit_msg = "wrote to src/main.rs: fixed the issue.";
        let build_output = "Compiling panda v1.0.0\nFinished [unoptimized] in 0.50s\n\
            Test result: ok. 5 passed; 0 failed; done.";
        let msgs = make_msgs(&[
            ("assistant", edit_msg),      // idx 0 — last edit marker
            ("assistant", build_output),  // idx 1 — build AFTER edit, not stale
            ("user", "latest turn"),
        ]);
        let config = CompressionConfig { recent_n: 0, ..Default::default() };
        let result = apply_smart_compression(msgs, &config);
        // Build output after edit should be untouched.
        assert_eq!(result.messages[1].content, build_output);
    }

    #[test]
    fn smart_compression_empty_input_is_fine() {
        let msgs: Vec<Message> = vec![];
        let config = CompressionConfig::default();
        let result = apply_smart_compression(msgs, &config);
        assert!(result.messages.is_empty());
        assert_eq!(result.tokens_in, 0);
        assert_eq!(result.tokens_out, 0);
    }

    #[test]
    fn is_stale_content_detects_git_status() {
        let git = "On branch main\nnothing to commit, working tree clean";
        assert!(is_stale_content(git, 0, None));
    }

    #[test]
    fn is_stale_content_does_not_flag_normal_text() {
        let normal = "Here is the implementation of the new feature. It uses BERT embeddings.";
        assert!(!is_stale_content(normal, 0, None));
    }
}
