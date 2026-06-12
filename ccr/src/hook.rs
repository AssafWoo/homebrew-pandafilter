use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::{self, Read};
use dirs;

#[derive(Debug, Deserialize)]
struct HookInput {
    #[serde(default)]
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
    stdout: String,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct HookOutput {
    output: String,
}

/// Core processing: takes raw hook stdin JSON, returns the JSON string to print
/// (already serialised HookOutput), or `None` for pass-through.
///
/// Does NOT attempt the daemon socket — call this directly from the daemon
/// server and from the fallback path in `run()`.
pub fn process(input: &str) -> Result<Option<String>> {
    crate::bert_budget::reset();
    let hook_input: HookInput = match serde_json::from_str(input) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };

    match hook_input.tool_name.as_str() {
        "Read" => process_read(hook_input),
        "Edit" => process_edit(hook_input),
        "Glob" => process_glob(hook_input),
        "Grep" => process_grep(hook_input),
        "WebFetch" => process_webfetch(hook_input),
        "WebSearch" => process_websearch(hook_input),
        _ => process_bash(hook_input), // Bash and unknown tools
    }
}

/// Entry point for lifecycle hooks that don't carry tool I/O.
/// `subcommand` is one of: "compact-capture", "compact-restore".
pub fn run_lifecycle(subcommand: &str) -> Result<()> {
    match subcommand {
        "compact-capture" => process_compact_capture(),
        "compact-restore" => process_compact_restore(),
        _ => Ok(()),
    }
}

pub fn run() -> Result<()> {
    // Overlap one-time tokenizer init with stdin reading / filtering.
    std::thread::spawn(panda_core::tokens::warm);

    // Integrity check: warn and exit if hook script has been tampered with.
    // PANDA_AGENT env var is set by Cursor's PostToolUse hook command; default = claude.
    let agent = std::env::var("PANDA_AGENT").unwrap_or_else(|_| "claude".to_string());
    if let Some(home) = dirs::home_dir() {
        let (script, hashdir) = match agent.as_str() {
            "cursor" => (
                home.join(".cursor").join("hooks").join("panda-rewrite.sh"),
                home.join(".cursor").join("hooks"),
            ),
            "codex" => (
                home.join(".codex").join("panda-rewrite.sh"),
                home.join(".codex"),
            ),
            "windsurf" => (
                home.join(".codeium").join("windsurf").join("panda-rewrite.sh"),
                home.join(".codeium").join("windsurf"),
            ),
            _ => (
                home.join(".claude").join("hooks").join("panda-rewrite.sh"),
                home.join(".claude").join("hooks"),
            ),
        };
        crate::integrity::runtime_check(&script, &hashdir);
    }

    let mut raw = String::new();
    if io::stdin().read_to_string(&mut raw).is_err() {
        return Ok(());
    }

    if let Ok(Some(output)) = process(&raw) {
        print!("{}", output);
    }
    Ok(())
}

// ── Bash tool handler ─────────────────────────────────────────────────────────

fn process_bash(hook_input: HookInput) -> Result<Option<String>> {
    let _start = std::time::Instant::now();

    // Proactively ensure the daemon is running — fire-and-forget, costs ~1ms.
    // While the hook does handler routing and regex filtering (50-200ms of work),
    // the daemon has a head start loading its ONNX model so it's warm by the time
    // the first embed call arrives. If already running, panda daemon start exits
    // immediately (idempotent). No-op on non-Unix builds.
    #[cfg(unix)]
    if !panda_core::embed_client::daemon_ping() {
        std::thread::spawn(|| {
            panda_core::embed_client::try_auto_start_detached();
        });
    }

    let full_cmd = hook_input
        .tool_input
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Skip commands that already went through cmd/run.rs or cmd/filter.rs —
    // those paths record their own analytics and the output is already compressed.
    if full_cmd.trim_start().starts_with("panda ") {
        return Ok(None);
    }

    // Resolve the effective command for attribution:
    // 1. Skip leading comment/blank lines — e.g. "# check status\ngit status" → "git status"
    // 2. Strip no-output prefixes — e.g. "sleep 2 && cargo test" → "cargo test"
    // This ensures the right handler is selected and analytics are attributed correctly.
    let effective_cmd: &str = {
        let first_real = full_cmd.lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with('#'))
            .unwrap_or(full_cmd.trim());

        // If line starts with a no-output command and has && or ; separator, skip past it.
        let first_word_base = first_real
            .split_whitespace()
            .next()
            .map(|w| std::path::Path::new(w).file_name()
                .and_then(|n| n.to_str()).unwrap_or(w))
            .unwrap_or("");

        if is_no_output_cmd(first_word_base) {
            if let Some(pos) = first_real.find("&&") {
                let after = first_real[pos + 2..].trim();
                if !after.is_empty() { after } else { first_real }
            } else if let Some(pos) = first_real.find(';') {
                let after = first_real[pos + 1..].trim();
                if !after.is_empty() && !after.starts_with('#') { after } else { first_real }
            } else {
                first_real
            }
        } else {
            first_real
        }
    };

    // If command was rewritten by a wrapper (e.g. RTK: "rtk git status"),
    // attribute analytics to the real underlying command, not the wrapper.
    // Also normalize full paths to basename: "/usr/bin/git" → "git".
    // Skip leading KEY=VALUE env var assignments (e.g. "GIT_COMMITTER_NAME=Assaf git commit").
    let command_hint = {
        let mut tokens = effective_cmd.split_whitespace()
            .skip_while(|t| {
                let eq = t.find('=').unwrap_or(0);
                eq > 0 && t[..eq].chars().all(|c| c.is_ascii_uppercase() || c == '_')
            });
        let first = tokens.next().unwrap_or("");
        let real = if first == "rtk" {
            tokens.next().unwrap_or("")
        } else {
            first
        };
        // Basename: strip path prefix so "/usr/bin/git" and "~/.cargo/bin/git" → "git"
        let basename = std::path::Path::new(real)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(real);
        if basename.is_empty() { None } else { Some(basename.to_string()) }
    };

    // Also catch full-path panda invocations like "/home/user/.cargo/bin/panda gain"
    // that don't start with the literal string "panda ".
    if command_hint.as_deref() == Some("panda") {
        return Ok(None);
    }

    let output_text = if let Some(err) = &hook_input.tool_response.error {
        err.clone()
    } else if !hook_input.tool_response.output.is_empty() {
        hook_input.tool_response.output.clone()
    } else {
        hook_input.tool_response.stdout.clone()
    };

    if output_text.is_empty() {
        return Ok(None);
    }

    // Skip the entire pipeline for small outputs.
    // Raised from 800 → 2000: outputs under ~500 tokens rarely benefit from BERT
    // compression vs the IPC overhead of a daemon embed call. Commands like
    // `git log --oneline`, `go list`, `wc`, `ls`, and most shell one-liners stay
    // under 2000 chars and pass through instantly.
    const BYPASS_CHARS: usize = 2000;
    // Compiler-error outputs must always be processed regardless of size so that
    // error-loop detection can record and compare signatures across retries.
    let has_compiler_errors = output_text.contains("error[E")
        || output_text.contains("error[W")
        || output_text.contains("error TS");
    if output_text.len() < BYPASS_CHARS && !has_compiler_errors {
        return Ok(None);
    }

    // Fix 3: skip shell infrastructure commands that never produce meaningful
    // output of their own. Any output attributed to these is leaked from a
    // compound command (e.g. `sleep 30 && curl ...`) and should pass through
    // unmodified rather than being logged as a low-savings run.
    // Guard: only skip when there are no error signals in the output.
    if command_hint.as_deref().map(is_no_output_cmd).unwrap_or(false)
        && !output_text.contains("error")
        && !output_text.contains("Error")
        && !output_text.contains("invalid")
    {
        return Ok(None);
    }

    let config = match crate::config_loader::load_config() {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };

    // IX: use the last 3 assistant messages as the BERT query when available.
    // Multi-turn accumulation weights by recency [200, 150, 100 chars] so that
    // repeated themes (e.g. "auth", "token refresh") dominate the query embedding.
    // Falls back to the command string if no session file is found.
    let query = crate::intent::extract_intent_multi(3).or_else(|| {
        hook_input
            .tool_input
            .get("command")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    });

    // Clean-room mode: PANDA_CLEAN_ROOM=1 bypasses all learned state (noise
    // store, result cache, session dedup, cross-command dedup) so the pipeline
    // can be evaluated fairly on a new repo without 1,800 learned patterns
    // interfering. Patterns are NOT deleted — just ignored for this invocation.
    let clean_room = std::env::var("PANDA_CLEAN_ROOM").as_deref() == Ok("1");

    let sid = crate::session::session_id();
    let mut session = crate::session::SessionState::load(&sid);

    // RC tier 1: per-session raw cache — byte-identical output on hit (prompt cache stability).
    // Loaded once here; kept in `rc_cache` so the insert at the end avoids a second disk read.
    let rc_key = crate::result_cache::ResultCache::compute_key(&output_text, command_hint.as_deref());
    let mut rc_cache: Option<crate::result_cache::ResultCache> = None;
    if !clean_room {
        let mut rc = crate::result_cache::ResultCache::load(&sid);
        rc.evict_old();
        if let Some(entry) = rc.lookup(&rc_key) {
            let cached_output = entry.output.clone();
            let analytics = panda_core::analytics::Analytics::new_cache_hit(
                entry.input_tokens,
                entry.output_tokens,
                command_hint.clone(),
                None,
            );
            crate::util::append_analytics(&analytics);
            let hook_output = HookOutput { output: cached_output };
            return Ok(Some(serde_json::to_string(&hook_output)?));
        }
        rc_cache = Some(rc);
    }

    // RC tier 2: cross-session global raw cache (24 h TTL) — compute reuse across sessions.
    // Checked only on per-session miss. Skips the full pipeline; output is byte-identical.
    let mut global_rc_cache: Option<crate::result_cache::ResultCache> = None;
    if !clean_room {
        let mut grc = crate::result_cache::ResultCache::load_global();
        grc.evict_global_old();
        if let Some(entry) = grc.lookup(&rc_key) {
            let cached_output = entry.output.clone();
            let analytics = panda_core::analytics::Analytics::new_cache_hit(
                entry.input_tokens,
                entry.output_tokens,
                command_hint.clone(),
                None,
            );
            crate::util::append_analytics(&analytics);
            // Also populate the per-session cache so future same-session hits skip this step.
            if let Some(ref mut rc) = rc_cache {
                rc.insert(rc_key.clone(), cached_output.clone(), entry.input_tokens, entry.output_tokens);
                let sid_bg = sid.clone();
                let rc_bg = rc_cache.take().unwrap();
                std::thread::spawn(move || { rc_bg.save(&sid_bg); });
            }
            return Ok(Some(serde_json::to_string(&HookOutput { output: cached_output })?));
        }
        global_rc_cache = Some(grc);
    }

    // cmd_key for session tracking: skip leading KEY=VALUE env vars and wrapper prefix.
    // "GIT_COMMITTER_NAME=Assaf git commit -m foo" → "git commit"
    // "rtk git status" → "git status"
    // Uses effective_cmd (comment-skipped, no-output-prefix-stripped) for consistency.
    let cmd_key: String = {
        fn is_env_assign(t: &&str) -> bool {
            let eq = t.find('=').unwrap_or(0);
            eq > 0 && t[..eq].chars().all(|c| c.is_ascii_uppercase() || c == '_')
        }
        let mut real_tokens = effective_cmd.split_whitespace().skip_while(is_env_assign);
        let first = real_tokens.next().unwrap_or("");
        let rest: Vec<&str> = real_tokens.collect();
        let real_iter: Box<dyn Iterator<Item = &str>> = if first == "rtk" {
            Box::new(rest.into_iter())
        } else {
            Box::new(std::iter::once(first).chain(rest.into_iter()))
        };
        real_iter.take(2).collect::<Vec<_>>().join(" ")
    };

    // RC tier 3: normalized-hash redirect — O(1), fires before BERT.
    // Strips temporal noise (timestamps, UUIDs, durations, git SHAs) then hashes.
    // On hit emits a short "output unchanged" marker (~15 tokens) instead of the
    // full compressed output, saving tokens for re-runs that differ only in noise.
    // Complements the BERT polling suppressor (which catches near-duplicates at 0.80
    // cosine threshold); this catches exact-after-normalization matches for free.
    let normalized_text = strip_temporal_noise(&output_text);
    let norm_key = crate::result_cache::ResultCache::compute_normalized_key(
        &normalized_text,
        command_hint.as_deref(),
    );
    if !clean_room {
        if let Some(ref rc) = rc_cache {
            if let Some(norm_entry) = rc.lookup_normalized(&norm_key) {
                let in_tok = panda_core::tokens::count_tokens(&output_text);
                let tokens_saved = norm_entry.output_tokens.saturating_sub(1);
                let marker = format!(
                    "[output unchanged since turn {} — ~{} tokens saved]",
                    norm_entry.turn_num, tokens_saved
                );
                crate::util::append_analytics(&panda_core::analytics::Analytics::new(
                    in_tok,
                    panda_core::tokens::count_tokens(&marker),
                    command_hint.clone(),
                    None,
                    None,
                ));
                return Ok(Some(serde_json::to_string(&HookOutput { output: marker })?));
            }
        }
    }

    // Fix 1+2: Polling suppressor with BERT embeddings.
    // If the same command ran within the last 120 seconds, embed the
    // timestamp-stripped version and check similarity at 0.80.
    // Catches near-duplicates (shifted line numbers, changed counts) that the
    // normalized hash above misses. Normalized text already computed above.
    // Skipped in clean-room mode — session history is not consulted.
    if !clean_room && session.has_recent_entry(&cmd_key, 120) && crate::bert_budget::try_consume() {
        if let Ok(mut embs) = panda_core::summarizer::embed_batch(&[normalized_text.as_str()]) {
            if let Some(emb) = embs.pop() {
                if let Some(hit) = session.find_similar_recent(&cmd_key, &emb) {
                    let age = crate::session::format_age(hit.age_secs);
                    let marker = format!(
                        "[same output as turn {} ({} ago) — {} tokens saved]",
                        hit.turn, age, hit.tokens_saved
                    );
                    let in_tok = panda_core::tokens::count_tokens(&output_text);
                    let out_tok = panda_core::tokens::count_tokens(&marker);
                    crate::util::append_analytics(&panda_core::analytics::Analytics::new(
                        in_tok, out_tok, command_hint.clone(), None, None,
                    ));
                    return Ok(Some(serde_json::to_string(&HookOutput { output: marker })?));
                }
            }
        }
    }

    // In clean-room mode, ignore all session-derived pressure so the pipeline
    // uses only the raw config pressure (no learned stale-context boosting).
    let historical_centroid = if clean_room { None } else { session.command_centroid(&cmd_key).cloned() };

    let stability_pressure = if clean_room {
        0.0
    } else {
        // Adaptive pressure: if the same command has been producing near-identical output
        // (high centroid_delta similarity), it's stale context — compress it more.
        session
            .last_centroid_delta(&cmd_key)
            .map(stability_to_pressure)
            .unwrap_or(0.0)
    };
    let staleness_pressure = if clean_room { 0.0 } else { session.staleness_pressure() };
    let pressure =
        (session.context_pressure() + stability_pressure + staleness_pressure).min(1.0);
    panda_core::zoom::enable();

    // NL: apply pre-filter to remove lines promoted as permanent noise.
    // Skipped in clean-room mode so learned patterns don't affect evaluation.
    let project_key = crate::util::project_key();
    let noise_store = if clean_room {
        None
    } else {
        project_key
            .as_ref()
            .map(|k| crate::noise_learner::NoiseStore::load(k))
    };

    let raw_lines: Vec<&str> = output_text.lines().collect();
    let filtered_text: String = if let Some(ref store) = noise_store {
        let kept = store.apply_pre_filter(&raw_lines);
        if kept.len() < raw_lines.len() {
            kept.join("\n")
        } else {
            output_text.clone()
        }
    } else {
        output_text.clone()
    };

    // FB: feedback-tuned keep threshold. Commands whose zoom blocks keep
    // getting expanded are over-compressed → lower the keep threshold so more
    // lines survive. Skipped in clean-room mode to keep evaluations unbiased.
    if !clean_room {
        panda_core::summarizer::set_keep_threshold_scale(crate::feedback::keep_scale(&cmd_key));
    }

    let pipeline = panda_core::pipeline::Pipeline::new(config.with_pressure(pressure));
    let mut result = match pipeline.process(
        &filtered_text,
        command_hint.as_deref(),
        query.as_deref(),
        historical_centroid.as_deref(),
    ) {
        Ok(r) => r,
        Err(_) => return Ok(None),
    };

    // NL: record what the pipeline suppressed so we can learn project noise.
    // Skipped in clean-room mode — we don't want evaluation runs to poison the model.
    if !clean_room {
        if let (Some(ref key), Some(mut store)) = (&project_key, noise_store) {
            let output_lines: Vec<&str> = result.output.lines().collect();
            store.record_lines(&raw_lines, &output_lines);
            store.promote_eligible();
            store.evict_stale(now_secs());
            let key_bg = key.clone();
            std::thread::spawn(move || { store.save(&key_bg); });
        }
    }

    // Enrich zoom block labels with content-derived categories before persisting.
    result.output = enrich_zoom_labels(&result.output, &result.zoom_blocks);
    let _ = crate::zoom_store::save_blocks(&sid, result.zoom_blocks, Some(&cmd_key));

    // ── Error-loop detection ──────────────────────────────────────────────────
    // Parse error signatures from pipeline output BEFORE any session-aware passes.
    // Stored early so they reflect the actual errors, not compressed summaries.
    let current_error_set = crate::error_signatures::ErrorSet::from_output(&result.output);

    // ── Cross-session knowledge (KS) ─────────────────────────────────────────
    // Errors recur across sessions; their fixes shouldn't be rediscovered.
    // When a known, previously-resolved error reappears, inject a one-line
    // hint. When this run clears errors the previous run of the same command
    // had, record the session's edited files as the resolution.
    if !clean_room {
        if let Some(hint) = apply_knowledge_store(&cmd_key, &current_error_set, &session) {
            result.output = format!("{}\n{}", hint, result.output);
        }
    }

    // If the same command has produced overlapping errors before, replace output
    // with a structural diff (fixed / new / unchanged). Falls through to C3 when:
    //   - no errors in output
    //   - no prior run with errors for this command
    //   - all current errors are new (first encounter of this error set)
    if let Some(loop_output) = crate::error_signatures::apply_error_loop_detection(
        &result.output,
        &cmd_key,
        &session,
    ) {
        let errloop_blocks = panda_core::zoom::drain();
        if !errloop_blocks.is_empty() {
            let _ = crate::zoom_store::save_blocks(&sid, errloop_blocks, Some(&cmd_key));
        }
        result.output = loop_output;
    }

    // ── SimHash near-duplicate dedup ─────────────────────────────────────────────
    // For outputs with many similar lines (logs, repeated errors), SimHash groups
    // near-duplicates in O(n) without BERT overhead. Only activates for 50+ line
    // outputs where repetition saves tokens.
    {
        let line_count = result.output.lines().count();
        if line_count >= panda_core::simhash::MIN_LINES {
            let lines: Vec<&str> = result.output.lines().collect();
            let deduped = panda_core::simhash::dedup_near_duplicates(
                &lines,
                panda_core::simhash::HAMMING_THRESHOLD,
            );
            if deduped.len() < lines.len() {
                result.output = deduped.join("\n");
            }
        }
    }

    // ── Handler fast path + graduated bypass ────────────────────────────────
    //
    // BERT-based session passes (delta, dedup, cross-dedup, C2 compression) add
    // latency per command via embedding calls. For well-handled short output, BERT
    // can't improve on what the handler already did — skip it.
    //
    // Graduated bypass (thresholds raised to reduce unnecessary BERT calls):
    //   handler_fast_path: handler compressed >15% AND output < 5000 chars → skip BERT
    //   soft_bypass: handler output < 2500 chars AND barely compressed (<10%) → skip BERT
    //   Neither: run full BERT pipeline (large/unhandled output)
    const BERT_MIN_LINES: usize = 30;
    let pipeline_line_count = result.output.lines().count();

    let handler_fast_path = command_hint.is_some()
        && result.output.len() < 5000
        && (result.output.len() as f64) < (output_text.len() as f64 * 0.85);

    let soft_bypass = !handler_fast_path
        && result.output.len() < 2500
        && (result.output.len() as f64 / output_text.len().max(1) as f64) > 0.90;

    // Turn-1 bypass: on the very first command of a session there is no centroid
    // or prior embedding to compare against, so the delta and dedup passes are
    // meaningless — they would record state but produce zero compression benefit.
    let first_turn = session.entries.is_empty();

    let skip_bert = clean_room || handler_fast_path || soft_bypass || first_turn;

    let mut final_output = if skip_bert {
        // Fast path: use handler output directly, skip all BERT-based passes.
        // Still record a lightweight session entry (no embedding) to maintain turn count.
        if !clean_room {
            let tokens = panda_core::tokens::count_tokens(&result.output);
            let zero_emb = vec![0.0f32; 384];
            let is_state = command_hint.as_deref()
                .and_then(|h| crate::config_loader::load_config().ok().map(|c| (h.to_string(), c)))
                .map(|(h, cfg)| cfg.global.state_commands.iter().any(|s| s == &h))
                .unwrap_or(false);
            session.record(&cmd_key, zero_emb, tokens, &result.output, is_state, None);
            if !current_error_set.is_empty() {
                session.set_last_error_signatures(&cmd_key, current_error_set.to_storage());
            }
            let sess_bg = session.clone();
            let sid_bg = sid.clone();
            std::thread::spawn(move || { sess_bg.save(&sid_bg); });
        }
        result.output.clone()
    } else {
        // ── Full BERT pipeline ───────────────────────────────────────────────
        let pipeline_emb = if pipeline_line_count >= BERT_MIN_LINES && crate::bert_budget::try_consume() {
            panda_core::summarizer::embed_batch(&[result.output.as_str()])
                .ok()
                .and_then(|mut v| v.pop())
        } else {
            None
        };

        let output_after_delta = if let Some(ref emb) = pipeline_emb {
            let lines: Vec<&str> = result.output.lines().collect();
            session
                .compute_delta(&cmd_key, &lines, emb)
                .map(|d| d.output)
                .unwrap_or_else(|| result.output.clone())
        } else {
            result.output.clone()
        };

        let after_dedup = if clean_room {
            output_after_delta.clone()
        } else {
            apply_sentence_dedup(&output_after_delta, &cmd_key, &session)
        };

        let is_state = {
            if let Ok(cfg) = crate::config_loader::load_config() {
                cfg.global.state_commands.iter().any(|s| {
                    command_hint.as_deref() == Some(s.as_str())
                })
            } else {
                false
            }
        };

        let after_xdedup = if clean_room {
            after_dedup
        } else {
            cross_command_dedup(&after_dedup, &cmd_key, &session, &sid, is_state)
        };

        let xdedup_blocks = panda_core::zoom::drain();
        if !xdedup_blocks.is_empty() {
            let _ = crate::zoom_store::save_blocks(&sid, xdedup_blocks, Some(&cmd_key));
        }

        let compression_factor = if clean_room { 1.0 } else { session.compression_factor() };
        let centroid_for_c2 = if clean_room { None } else { session.command_centroid(&cmd_key).cloned() };
        let bert_output = if compression_factor < 0.90 && pipeline_line_count >= BERT_MIN_LINES {
            let line_count = after_xdedup.lines().count();
            let reduced_budget = ((line_count as f32 * compression_factor) as usize).max(10);
            if let Some(ref centroid) = centroid_for_c2 {
                panda_core::summarizer::summarize_against_centroid(&after_xdedup, reduced_budget, centroid)
                    .output
            } else {
                panda_core::summarizer::summarize(&after_xdedup, reduced_budget).output
            }
        } else {
            after_xdedup
        };

        // Session update: record embeddings/centroid/errors for next-turn dedup.
        if !clean_room && pipeline_line_count >= BERT_MIN_LINES && crate::bert_budget::try_consume() {
            let non_empty: Vec<&str> = bert_output
                .lines()
                .filter(|l| !l.trim().is_empty())
                .collect();
            if let Ok(line_embeddings) = panda_core::summarizer::embed_batch(&non_empty) {
                let tokens = panda_core::tokens::count_tokens(&bert_output);
                let new_centroid =
                    panda_core::summarizer::compute_centroid_from_embeddings(&line_embeddings);
                let emb = new_centroid.clone();
                let centroid_delta = historical_centroid
                    .as_ref()
                    .map(|hist| crate::handlers::util::cosine_similarity(hist, &new_centroid));
                session.update_command_centroid(&cmd_key, new_centroid);
                session.record(&cmd_key, emb, tokens, &bert_output, is_state, centroid_delta);
                if !current_error_set.is_empty() {
                    session.set_last_error_signatures(&cmd_key, current_error_set.to_storage());
                }
                let sess_bg = session.clone();
                let sid_bg = sid.clone();
                std::thread::spawn(move || { sess_bg.save(&sid_bg); });
            }
        }

        bert_output
    };

    if pressure > 0.80 {
        final_output.push_str(
            "\n[⚠ context near full — run `panda compress --scan-session --dry-run` to estimate savings, or `panda compress --scan-session` to compress]",
        );
    }

    // ── Focus boost ───────────────────────────────────────────────────────────
    // Reorder lines so content about actively-edited/read files appears first.
    // Runs after all handler/BERT compression so it operates on the final text.
    final_output = apply_focus_boost(&final_output, &session, clean_room);

    // Record analytics: use pipeline output tokens (pre-BERT) for input — accurate
    // enough and avoids a BERT dependency for analytics correctness.
    let input_tokens = panda_core::tokens::count_tokens(&output_text);
    let output_tokens = panda_core::tokens::count_tokens(&final_output);
    // subcommand is the second non-flag token of the real command (already in cmd_key)
    let subcommand = cmd_key
        .split_whitespace()
        .nth(1)
        .filter(|s| !s.starts_with('-'))
        .map(|s| s.to_string());
    let analytics = panda_core::analytics::Analytics::new(
        input_tokens,
        output_tokens,
        command_hint.clone(),
        subcommand,
        Some(_start.elapsed().as_millis() as u64),
    );
    crate::util::append_analytics(&analytics);

    // ── Tee file + overflow hint ────────────────────────────────────────────────
    // Write the raw (pre-compression) output to a tee file so the agent can
    // retrieve full context when needed. Append a recovery hint when compression
    // is aggressive (>60% savings), so the agent knows where to look.
    let tee_path = write_hook_tee(&cmd_key, &output_text);
    let input_chars = output_text.len();
    let output_chars = final_output.len();
    let savings_pct = 100usize.saturating_sub(output_chars.saturating_mul(100) / input_chars.max(1));

    if savings_pct > 60 {
        if let Some(ref path) = tee_path {
            final_output.push_str(&format!("\n[full output: {}]", path.display()));
        }
    }

    // ── Transparency header ───────────────────────────────────────────────────
    // Prepend a single-line comment showing what happened. Lets the agent (and
    // the developer inspecting traces) see which handler fired and how much was
    // removed.
    //
    // Only added when compression exceeds MIN_HEADER_SAVINGS_PCT (15%). Below
    // that threshold the ~55-char header consumes more than the compression saved,
    // producing a net-negative result (confirmed by benchmark: Build category −3.0%).
    //
    // Added BEFORE the cache insert so the cache stores the header-inclusive bytes,
    // ensuring cache-hit responses are byte-identical to first-run responses.
    const MIN_HEADER_SAVINGS_PCT: usize = 15;
    if output_chars < input_chars && savings_pct >= MIN_HEADER_SAVINGS_PCT {
        let ratio_pct = 100 - savings_pct;
        let handler_label = command_hint.as_deref().unwrap_or("generic");
        let elapsed_ms = _start.elapsed().as_millis() as u64;
        let header = format!(
            "# panda: {} → {} chars ({} handler; {}% retained; {}ms)\n",
            input_chars, output_chars, handler_label, ratio_pct, elapsed_ms
        );
        final_output = format!("{}{}", header, final_output);
    }

    // ── Trust warnings ────────────────────────────────────────────────────────
    // If load_user_filters() found an untrusted project filter file, it pushed
    // a warning into filter_trust::PENDING_WARNINGS. Drain and prepend here so
    // the LLM sees the security notice in its tool output.
    let trust_warnings = crate::filter_trust::take_warnings();
    if !trust_warnings.is_empty() {
        let warning_block = trust_warnings.join("\n");
        final_output = format!("{}\n{}", warning_block, final_output);
    }

    // Result cache inserts — per-session raw + normalized redirect + global raw.
    // All saved asynchronously after output is returned (no blocking disk I/O on
    // the hot path). The per-session save carries norm_entries; the global save
    // strips them (turn numbers are meaningless across sessions).
    let turn_num = session.entries.len() + 1;
    if let Some(mut rc) = rc_cache {
        rc.insert(rc_key.clone(), final_output.clone(), input_tokens, output_tokens);
        rc.insert_normalized(norm_key, turn_num, output_tokens);
        let sid_bg = sid.clone();
        std::thread::spawn(move || { rc.save(&sid_bg); });
    }
    if let Some(mut grc) = global_rc_cache {
        grc.insert(rc_key, final_output.clone(), input_tokens, output_tokens);
        std::thread::spawn(move || { grc.save_global(); });
    }

    let hook_output = HookOutput { output: final_output };
    Ok(Some(serde_json::to_string(&hook_output)?))
}

// ── Focus-aware compression ───────────────────────────────────────────────────

/// Attempt focus-aware compression for a large code file.
/// Returns `Some((output, result))` if focus is active and compression was applied.
/// Returns `None` to fall through to the normal pipeline.
fn try_focus_compress(
    file_path: &str,
    content: &str,
    session: &crate::session::SessionState,
    _line_count: usize,
) -> Option<(String, crate::handlers::focus_compress::FocusCompressResult)> {
    // 1. Get prompt embedding (gated on focus being active)
    let prompt_emb = session.command_centroid("(focus_prompt)")?.clone();

    // 2. File must be a code file (check extension)
    let ext = std::path::Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    if !matches!(
        ext.to_lowercase().as_str(),
        "rs" | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "mjs"
            | "cjs"
            | "go"
            | "java"
            | "cs"
            | "cpp"
            | "cc"
            | "c"
            | "h"
            | "hpp"
            | "py"
            | "pyi"
    ) {
        return None;
    }

    // 3. Enable zoom for registering blocks
    panda_core::zoom::enable();

    // 4. Split into sections
    let sections = crate::handlers::focus_compress::split_into_sections(content, ext);

    if sections.len() < 2 {
        return None;
    }

    // 5. Get edit-preserve ranges
    let preserve_ranges = session.edit_preserve_ranges(file_path, 20);

    // 6. Score and compress
    let result = crate::handlers::focus_compress::score_and_compress(
        &sections,
        &prompt_emb,
        &preserve_ranges,
    )
    .ok()?;

    if result.sections_compressed == 0 {
        return None;
    }

    let output = result.output.clone();
    Some((output, result))
}

fn check_focus_edit_hit(session_id: &str, file_path: &str, edit_line: usize) {
    if let Ok(conn) = crate::analytics_db::open() {
        let result: rusqlite::Result<String> = conn.query_row(
            "SELECT section_details FROM focus_compression_events \
             WHERE session_id = ?1 AND file_path = ?2 \
             ORDER BY timestamp_secs DESC LIMIT 1",
            rusqlite::params![session_id, file_path],
            |row| row.get(0),
        );

        if let Ok(details_json) = result {
            if let Ok(details) =
                serde_json::from_str::<Vec<(usize, usize, bool)>>(&details_json)
            {
                let was_preserved = details.iter().any(|(start, end, preserved)| {
                    *preserved && edit_line >= *start && edit_line < *end
                });
                let _ = crate::analytics_db::record_focus_edit_hit(
                    session_id,
                    file_path,
                    edit_line,
                    was_preserved,
                );
            }
        }
    }
}

// ── Read tool handler ─────────────────────────────────────────────────────────

fn process_read(hook_input: HookInput) -> Result<Option<String>> {
    let file_path = hook_input
        .tool_input
        .get("file_path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let output_text = if !hook_input.tool_response.output.is_empty() {
        hook_input.tool_response.output.clone()
    } else {
        hook_input.tool_response.stdout.clone()
    };

    if output_text.is_empty() {
        return Ok(None);
    }

    // Binary file guard: if output contains null bytes, pass through unchanged
    if output_text.bytes().any(|b| b == 0) {
        return Ok(None);
    }

    // Short files pass through without compression
    let line_count = output_text.lines().count();
    if line_count < 50 {
        return Ok(None);
    }

    let config = match crate::config_loader::load_config() {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };

    // Aggressive read mode early-exit — bypasses BERT pipeline entirely
    {
        use panda_core::config::ReadMode;
        if config.read.mode != ReadMode::Passthrough {
            use crate::handlers::{Handler, read::ReadHandlerLevel};
            let handler = ReadHandlerLevel::from_read_mode(&config.read.mode);
            let filtered = handler.filter(&output_text, &[file_path.clone()]);
            let in_tok  = panda_core::tokens::count_tokens(&output_text);
            let out_tok = panda_core::tokens::count_tokens(&filtered);
            crate::util::append_analytics(&panda_core::analytics::Analytics::new(
                in_tok, out_tok, Some("(read-level)".to_string()), None, None,
            ));
            return Ok(Some(serde_json::to_string(&HookOutput { output: filtered })?));
        }
    }

    // ── Delta / structural mode ───────────────────────────────────────────────
    // Check if this file was read earlier in the session.
    // If so, send a compact diff or structural skeleton instead of the full file.
    {
        let sid_delta = crate::session::session_id();
        let mut session_delta = crate::session::SessionState::load(&sid_delta);

        // Get current mtime from filesystem
        let current_mtime: u64 = std::fs::metadata(&file_path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);

        if let Some((cached_mtime, cached_content)) = session_delta.get_file_cache(&file_path) {
            if cached_mtime == current_mtime || cached_content == output_text.as_str() {
                // File unchanged — return structural digest
                let digest = panda_core::structure_map::extract(&file_path, &output_text);
                let in_tok = panda_core::tokens::count_tokens(&output_text);
                let out_tok = panda_core::tokens::count_tokens(&digest);
                crate::util::append_analytics(&panda_core::analytics::Analytics::new(
                    in_tok,
                    out_tok,
                    Some("(read-structural)".to_string()),
                    None,
                    None,
                ));
                let _ = crate::analytics_db::record_session_read(&sid_delta, &file_path, in_tok);
                return Ok(Some(serde_json::to_string(&HookOutput { output: digest })?));
            } else {
                // File changed — try delta diff
                let cached_owned = cached_content.to_string();
                match panda_core::delta::compute(&file_path, &cached_owned, &output_text) {
                    panda_core::delta::DeltaResult::Diff(diff) => {
                        let in_tok = panda_core::tokens::count_tokens(&output_text);
                        let out_tok = panda_core::tokens::count_tokens(&diff);
                        crate::util::append_analytics(&panda_core::analytics::Analytics::new(
                            in_tok,
                            out_tok,
                            Some("(read-delta)".to_string()),
                            None,
                            None,
                        ));
                        let _ = crate::analytics_db::record_session_read(&sid_delta, &file_path, in_tok);
                        // Update cache with new content
                        session_delta.set_file_cache(&file_path, current_mtime, &output_text);
                        session_delta.save(&sid_delta);
                        return Ok(Some(serde_json::to_string(&HookOutput { output: diff })?));
                    }
                    // TooLarge or NotEligible: fall through to normal pipeline
                    panda_core::delta::DeltaResult::Unchanged => {
                        let digest = panda_core::structure_map::extract(&file_path, &output_text);
                        let in_tok = panda_core::tokens::count_tokens(&output_text);
                        let out_tok = panda_core::tokens::count_tokens(&digest);
                        crate::util::append_analytics(&panda_core::analytics::Analytics::new(
                            in_tok, out_tok, Some("(read-structural)".to_string()), None, None,
                        ));
                        let _ = crate::analytics_db::record_session_read(&sid_delta, &file_path, in_tok);
                        return Ok(Some(serde_json::to_string(&HookOutput { output: digest })?));
                    }
                    _ => {
                        // TooLarge / NotEligible — update cache and fall through
                        session_delta.set_file_cache(&file_path, current_mtime, &output_text);
                        session_delta.save(&sid_delta);
                    }
                }
            }
        } else {
            // First read: store in cache for future delta comparisons
            session_delta.set_file_cache(&file_path, current_mtime, &output_text);
            session_delta.save(&sid_delta);
        }
    }

    // Use file extension as command hint, intent as query
    let ext_hint = std::path::Path::new(&file_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_string());

    let query = crate::intent::extract_intent().or_else(|| {
        std::path::Path::new(&file_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
    });

    let sid = crate::session::session_id();
    let mut session = crate::session::SessionState::load(&sid);
    let historical_centroid = session.command_centroid(&file_path).cloned();
    let pressure = session.context_pressure();

    // ── Edit-aware re-read: show only edited regions + context ────────────────
    // If the session has edit records for this file AND it's a re-read (file was
    // already in the session's command centroids — i.e. we've seen it before),
    // send just the edited sections instead of the full file.
    // Skipped for short files (<80 lines) where full content is cheap.
    if line_count >= 80 && historical_centroid.is_some() {
        let edit_ranges = session.edit_preserve_ranges(&file_path, 5);
        if !edit_ranges.is_empty() {
            let lines: Vec<&str> = output_text.lines().collect();
            let mut out_lines: Vec<String> = Vec::new();
            let mut last_end = 0usize;
            for (start, end) in &edit_ranges {
                let from = start.saturating_sub(1); // convert to 0-indexed
                let to = (*end).min(lines.len());
                if from > last_end + 1 {
                    out_lines.push(format!("[... {} lines ...]", from - last_end));
                }
                for (i, line) in lines[from..to].iter().enumerate() {
                    out_lines.push(format!("{}\t{}", from + i + 1, line));
                }
                last_end = to;
            }
            if last_end < lines.len() {
                out_lines.push(format!("[... {} lines ...]", lines.len() - last_end));
            }
            out_lines.push(format!(
                "[{} lines total, showing {} edited region(s) + context]",
                lines.len(),
                edit_ranges.len()
            ));

            let output = out_lines.join("\n");
            let in_tok = panda_core::tokens::count_tokens(&output_text);
            let out_tok = panda_core::tokens::count_tokens(&output);
            crate::util::append_analytics(&panda_core::analytics::Analytics::new(
                in_tok,
                out_tok,
                Some("(read-edit-ranges)".to_string()),
                None,
                None,
            ));
            return Ok(Some(serde_json::to_string(&HookOutput { output })?));
        }
    }

    // Focus-aware compression: replaces head/tail for large code files when focus is active
    if line_count >= 150 {
        if let Some((focus_output, focus_result)) =
            try_focus_compress(&file_path, &output_text, &session, line_count)
        {
            let zoom_blocks = panda_core::zoom::drain();
            let _ = crate::zoom_store::save_blocks(&sid, zoom_blocks, Some("(read)"));

            if crate::bert_budget::try_consume() {
                if let Ok(mut embs) =
                    panda_core::summarizer::embed_batch(&[focus_output.as_str()])
                {
                    if let Some(emb) = embs.pop() {
                        let tokens = panda_core::tokens::count_tokens(&focus_output);
                        session.record(&file_path, emb, tokens, &focus_output, false, None);
                        session.save(&sid);
                    }
                }
            }

            let input_tokens = panda_core::tokens::count_tokens(&output_text);
            let output_tokens = panda_core::tokens::count_tokens(&focus_output);
            let analytics = panda_core::analytics::Analytics::new(
                input_tokens,
                output_tokens,
                Some("(read-focus)".to_string()),
                None,
                None,
            );
            crate::util::append_analytics(&analytics);
            let _ = crate::analytics_db::record_session_read(&sid, &file_path, input_tokens);
            let _ = crate::analytics_db::record_focus_compression(
                &sid,
                &file_path,
                &focus_result,
                &crate::util::project_key().unwrap_or_default(),
            );
            refresh_focus_embedding(&file_path);

            return Ok(Some(serde_json::to_string(&HookOutput { output: focus_output })?));
        }
    }

    panda_core::zoom::enable();
    let pipeline = panda_core::pipeline::Pipeline::new(config.with_pressure(pressure));
    let result = match pipeline.process(
        &output_text,
        ext_hint.as_deref(),
        query.as_deref(),
        historical_centroid.as_deref(),
    ) {
        Ok(r) => r,
        Err(_) => return Ok(None),
    };

    let _ = crate::zoom_store::save_blocks(&sid, result.zoom_blocks, Some("(read)"));

    // 3.3: Edit-aware compression — preserve context around recently-edited areas
    let pipeline_output = {
        let preserve_ranges = session.edit_preserve_ranges(&file_path, 20);
        if !preserve_ranges.is_empty() {
            apply_edit_aware_compression(&result.output, &preserve_ranges)
        } else {
            result.output
        }
    };

    // 3.2: Cross-file dedup — collapse sections already seen in recently-read files
    let pipeline_output = apply_cross_file_dedup(&pipeline_output, &mut session, &sid);

    // Session dedup using file_path as cmd_key.
    // Threshold scales by file size — see `read_dedup_threshold()`.
    let compressed = if crate::bert_budget::try_consume() {
        if let Ok(mut embs) =
            panda_core::summarizer::embed_batch(&[pipeline_output.as_str()])
        {
            if let Some(emb) = embs.pop() {
                let tokens = panda_core::tokens::count_tokens(&pipeline_output);
                let line_count = pipeline_output.lines().count();
                let threshold = read_dedup_threshold(line_count);
                if let Some(hit) = session.find_similar_with_threshold(&file_path, &emb, threshold) {
                    let age = crate::session::format_age(hit.age_secs);
                    format!(
                        "[same file content as turn {} ({} ago) — {} tokens saved]",
                        hit.turn, age, hit.tokens_saved
                    )
                } else {
                    session.record(&file_path, emb, tokens, &pipeline_output, false, None);
                    session.save(&sid);
                    pipeline_output
                }
            } else {
                pipeline_output
            }
        } else {
            pipeline_output
        }
    } else {
        pipeline_output
    };

    let input_tokens = panda_core::tokens::count_tokens(&output_text);
    let output_tokens = panda_core::tokens::count_tokens(&compressed);
    let analytics = panda_core::analytics::Analytics::new(input_tokens, output_tokens, Some("(read)".to_string()), None, None);
    crate::util::append_analytics(&analytics);

    // Record this read in the session for focus precision tracking
    let _ = crate::analytics_db::record_session_read(&sid, &file_path, input_tokens);

    // 2.3: Refresh focus graph embedding with the fresh pipeline embedding
    refresh_focus_embedding(&file_path);

    let hook_output = HookOutput { output: compressed };
    Ok(Some(serde_json::to_string(&hook_output)?))
}

// ── Edit tool handler (3.3) ───────────────────────────────────────────────────

/// Track Edit events in the session so that subsequent re-reads of the same
/// file preserve context around the edited lines.
fn process_edit(hook_input: HookInput) -> Result<Option<String>> {
    let file_path = hook_input
        .tool_input
        .get("file_path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if file_path.is_empty() {
        return Ok(None);
    }

    // Try to figure out which lines were edited from the old_string
    // We can estimate the line range from the old_string content
    let old_string = hook_input
        .tool_input
        .get("old_string")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if old_string.is_empty() {
        return Ok(None);
    }

    // Find the line range of old_string in the file
    if let Ok(content) = std::fs::read_to_string(&file_path) {
        if let Some(byte_offset) = content.find(old_string) {
            let start_line = content[..byte_offset].lines().count();
            let edit_lines = old_string.lines().count();
            let end_line = start_line + edit_lines;

            let sid = crate::session::session_id();
            let mut session = crate::session::SessionState::load(&sid);
            session.record_edit(&file_path, start_line, end_line);
            // Invalidate the file content cache so the next read sends a fresh delta
            session.invalidate_file_cache(&file_path);
            session.save(&sid);

            // Focus edit-hit tracking: was this edit in a preserved or compressed section?
            check_focus_edit_hit(&sid, &file_path, start_line);
        }
    }

    Ok(None) // pass through — Edit output is not compressed
}

// ── Glob tool handler ─────────────────────────────────────────────────────────

fn process_glob(hook_input: HookInput) -> Result<Option<String>> {
    let pattern = hook_input
        .tool_input
        .get("pattern")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let output_text = if !hook_input.tool_response.output.is_empty() {
        hook_input.tool_response.output.clone()
    } else {
        hook_input.tool_response.stdout.clone()
    };

    if output_text.is_empty() {
        return Ok(None);
    }

    let paths: Vec<&str> = output_text.lines().filter(|l| !l.trim().is_empty()).collect();
    let total = paths.len();

    // Short results pass through
    if total <= 20 {
        return Ok(None);
    }

    let sid = crate::session::session_id();
    let mut session = crate::session::SessionState::load(&sid);
    let cmd_key = format!("glob:{}", pattern);

    // Session dedup: hash the exact path list
    let list_hash = crate::util::hash_str(&output_text);
    if let Some(entry) = session.entries.iter().rev().find(|e| e.cmd == cmd_key) {
        if entry.content_preview.starts_with(&list_hash) {
            let hook_output = HookOutput {
                output: format!(
                    "[same glob result as turn {} — {} paths]",
                    entry.turn, total
                ),
            };
            return Ok(Some(serde_json::to_string(&hook_output)?));
        }
    }

    // Group paths by parent directory
    let mut by_dir: std::collections::BTreeMap<String, Vec<&str>> =
        std::collections::BTreeMap::new();
    for path in &paths {
        let parent = std::path::Path::new(path)
            .parent()
            .and_then(|p| p.to_str())
            .unwrap_or(".")
            .to_string();
        by_dir.entry(parent).or_default().push(path);
    }

    let mut output_lines: Vec<String> = Vec::new();
    let mut shown = 0usize;
    const MAX_SHOWN: usize = 60;

    for (dir, files) in &by_dir {
        if shown >= MAX_SHOWN {
            break;
        }
        let remaining = MAX_SHOWN - shown;
        let show_count = files.len().min(remaining);
        for f in &files[..show_count] {
            output_lines.push(f.to_string());
        }
        if files.len() > show_count {
            output_lines.push(format!("  [+{} more in {}/]", files.len() - show_count, dir));
        }
        shown += show_count;
    }

    let hidden = total.saturating_sub(shown);
    if hidden > 0 {
        output_lines.push(format!("[+{} more paths not shown]", hidden));
    }
    output_lines.push(format!("[Glob: {} — {} paths total]", pattern, total));

    let compressed = output_lines.join("\n");

    // Record in session (use hash prefix as content_preview for dedup)
    let tokens = panda_core::tokens::count_tokens(&compressed);
    let preview = format!("{} {}", list_hash, &compressed[..compressed.len().min(3900)]);
    if crate::bert_budget::try_consume() {
        if let Ok(mut embs) = panda_core::summarizer::embed_batch(&[compressed.as_str()]) {
            if let Some(emb) = embs.pop() {
                session.entries.push(crate::session::SessionEntry {
                    turn: session.total_turns + 1,
                    cmd: cmd_key,
                    ts: now_secs(),
                    tokens,
                    embedding: emb,
                    content_preview: preview,
                    state_content: None,
                    centroid_delta: None,
                    error_signatures: None,
                });
                session.total_turns += 1;
                session.total_tokens += tokens;
                if session.entries.len() > 30 {
                    session.entries.remove(0);
                }
                session.save(&sid);
            }
        }
    }

    let input_tokens = panda_core::tokens::count_tokens(&output_text);
    let output_tokens = panda_core::tokens::count_tokens(&compressed);
    let analytics = panda_core::analytics::Analytics::new(input_tokens, output_tokens, Some("(glob)".to_string()), None, None);
    crate::util::append_analytics(&analytics);

    let hook_output = HookOutput { output: compressed };
    Ok(Some(serde_json::to_string(&hook_output)?))
}

// ── Cross-session knowledge store (KS) ───────────────────────────────────────

/// Record error occurrences/resolutions in the cross-session knowledge store
/// and return recurrence hint lines (joined) when a previously-resolved error
/// reappears. Best-effort: any storage failure returns None and is ignored.
fn apply_knowledge_store(
    cmd_key: &str,
    current_error_set: &crate::error_signatures::ErrorSet,
    session: &crate::session::SessionState,
) -> Option<String> {
    let conn = crate::knowledge::open().ok()?;
    let project = crate::analytics_db::current_project_path();

    if !current_error_set.is_empty() {
        let keys: Vec<String> = current_error_set
            .signatures
            .iter()
            .map(|s| s.key())
            .collect();

        // Hints first, so this occurrence doesn't inflate its own count.
        let hints = crate::knowledge::recurrence_hints(&conn, &project, &keys)
            .unwrap_or_default();

        let pairs: Vec<(String, String)> = current_error_set
            .signatures
            .iter()
            .map(|s| {
                // First raw line is the most readable one-line description.
                let display = s
                    .raw_lines
                    .first()
                    .cloned()
                    .unwrap_or_else(|| s.display());
                (s.key(), display.chars().take(200).collect())
            })
            .collect();
        let _ = crate::knowledge::record_errors(&conn, &project, &pairs);

        if hints.is_empty() {
            None
        } else {
            Some(hints.join("\n"))
        }
    } else {
        // No errors now — if the *immediately previous* run of this command
        // had errors, they were just resolved by whatever was edited this
        // session. Older errored entries don't count: their resolution was
        // already attributed when the first clean run followed them.
        let last_entry = session.entries.iter().rev().find(|e| e.cmd == cmd_key)?;
        let prior_sigs = last_entry.error_signatures.as_deref()?;
        let prior = crate::error_signatures::ErrorSet::from_storage(prior_sigs);
        if prior.is_empty() {
            return None;
        }
        let keys: Vec<String> = prior.signatures.iter().map(|s| s.key()).collect();
        let edited: Vec<String> = session.recent_edits.keys().cloned().collect();
        let _ = crate::knowledge::record_resolutions(&conn, &project, &keys, &edited);
        None
    }
}

// ── Grep tool handler ─────────────────────────────────────────────────────────

fn process_grep(hook_input: HookInput) -> Result<Option<String>> {
    let pattern = hook_input
        .tool_input
        .get("pattern")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let output_text = if !hook_input.tool_response.output.is_empty() {
        hook_input.tool_response.output.clone()
    } else {
        hook_input.tool_response.stdout.clone()
    };

    if output_text.is_empty() {
        return Ok(None);
    }

    // Short results pass through unchanged
    if output_text.lines().count() <= 10 {
        return Ok(None);
    }

    use crate::handlers::Handler;
    let handler = crate::handlers::grep::GrepHandler;
    let args: Vec<String> = vec!["grep".to_string(), pattern];
    let filtered = handler.filter(&output_text, &args);

    // Explore-gap dedup: collapse hit groups that fall inside file sections
    // the agent has already read this session.
    let filtered = apply_grep_context_dedup(&filtered);

    let input_tokens = panda_core::tokens::count_tokens(&output_text);
    let output_tokens = panda_core::tokens::count_tokens(&filtered);
    let analytics = panda_core::analytics::Analytics::new(
        input_tokens,
        output_tokens,
        Some("(grep-tool)".to_string()),
        None,
        None,
    );
    crate::util::append_analytics(&analytics);

    let hook_output = HookOutput { output: filtered };
    Ok(Some(serde_json::to_string(&hook_output)?))
}

// ── Grep context dedup (explore gap) ──────────────────────────────────────────

/// Minimum hits in a per-file group before it's worth an embedding call.
const GREP_DEDUP_MIN_GROUP_LINES: usize = 3;
/// Similarity to an already-read section that marks a hit group as redundant.
/// Higher than the cross-file dedup threshold (0.80): grep lines carry
/// path/line-number noise even after prefix stripping.
const GREP_DEDUP_SIM_THRESHOLD: f32 = 0.82;
/// Never suppress more than this fraction of the output — a grep where
/// everything is "already read" should still show the match locations.
const GREP_DEDUP_MAX_SUPPRESS_RATIO: f32 = 0.60;

/// Strip the `path:` / `path:lineno:` prefix from a grep content line so the
/// embedding compares match *content* against read file sections.
fn grep_line_content(line: &str) -> &str {
    let mut rest = line;
    for _ in 0..2 {
        match rest.split_once(':') {
            Some((head, tail))
                if !head.is_empty()
                    && (head.contains('/')
                        || head.contains('.')
                        || head.chars().all(|c| c.is_ascii_digit())) =>
            {
                rest = tail;
            }
            _ => break,
        }
    }
    rest
}

/// Leading file path of a grep output line, when the line has the
/// `path:...` shape. Used to group consecutive hits per file.
fn grep_line_path(line: &str) -> Option<&str> {
    let head = line.split(':').next()?;
    if head.is_empty() || head.contains(' ') {
        return None;
    }
    if head.contains('/') || head.contains('.') {
        Some(head)
    } else {
        None
    }
}

/// Collapse grep hit groups whose content the agent has already read this
/// session (cosine ≥ threshold against stored read-section embeddings).
/// Suppressed groups become one-line markers with a zoom expand ID.
fn apply_grep_context_dedup(filtered: &str) -> String {
    let sid = crate::session::session_id();
    let session = crate::session::SessionState::load(&sid);
    if session.read_section_embeddings.is_empty() {
        return filtered.to_string();
    }

    let lines: Vec<&str> = filtered.lines().collect();
    if lines.len() < 15 {
        return filtered.to_string();
    }

    // Group consecutive lines by file path prefix.
    // (path, start_idx, end_idx_exclusive)
    let mut groups: Vec<(String, usize, usize)> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        match grep_line_path(line) {
            Some(path) => match groups.last_mut() {
                Some((gpath, _, end)) if gpath == path && *end == i => *end = i + 1,
                _ => groups.push((path.to_string(), i, i + 1)),
            },
            None => {}
        }
    }

    let candidates: Vec<&(String, usize, usize)> = groups
        .iter()
        .filter(|(_, s, e)| e - s >= GREP_DEDUP_MIN_GROUP_LINES)
        .collect();
    if candidates.is_empty() {
        return filtered.to_string();
    }

    if !crate::bert_budget::try_consume() {
        return filtered.to_string();
    }
    let texts: Vec<String> = candidates
        .iter()
        .map(|(_, s, e)| {
            lines[*s..*e]
                .iter()
                .map(|l| grep_line_content(l))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect();
    let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
    let embeddings = match panda_core::summarizer::embed_batch(&text_refs) {
        Ok(e) => e,
        Err(_) => return filtered.to_string(),
    };

    let suppress_budget =
        (lines.len() as f32 * GREP_DEDUP_MAX_SUPPRESS_RATIO) as usize;
    let mut suppressed_lines = 0usize;
    let mut suppress: std::collections::HashMap<usize, String> =
        std::collections::HashMap::new();

    panda_core::zoom::enable();
    for ((path, s, e), emb) in candidates.iter().zip(embeddings.iter()) {
        let group_len = e - s;
        if suppressed_lines + group_len > suppress_budget {
            continue;
        }
        if session.is_section_seen(emb, GREP_DEDUP_SIM_THRESHOLD) {
            let zi = panda_core::zoom::register(
                lines[*s..*e].iter().map(|l| l.to_string()).collect(),
            );
            suppress.insert(
                *s,
                format!(
                    "[{}: {} matches in already-read context — panda expand {}]",
                    path, group_len, zi
                ),
            );
            suppressed_lines += group_len;
        }
    }

    let blocks = panda_core::zoom::drain();
    if suppress.is_empty() {
        return filtered.to_string();
    }
    let _ = crate::zoom_store::save_blocks(&sid, blocks, Some("(grep)"));

    // Rebuild: replace each suppressed group with its marker.
    let suppressed_ranges: std::collections::HashMap<usize, usize> = groups
        .iter()
        .filter(|(_, s, _)| suppress.contains_key(s))
        .map(|(_, s, e)| (*s, *e))
        .collect();

    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        if let Some(marker) = suppress.get(&i) {
            out.push(marker.clone());
            i = suppressed_ranges[&i];
        } else {
            out.push(lines[i].to_string());
            i += 1;
        }
    }
    out.join("\n")
}

// ── Sentence dedup (C1) ───────────────────────────────────────────────────────

fn apply_sentence_dedup(
    output: &str,
    _cmd: &str,
    session: &crate::session::SessionState,
) -> String {
    use panda_sdk::deduplicator::deduplicate;
    use panda_sdk::message::Message;

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

/// Cosine similarity threshold for Read dedup, scaled by filtered output length.
/// Longer outputs need higher similarity to trigger dedup because a small
/// edit barely moves the overall BERT embedding.
///
/// Calibrated against all-MiniLM-L6-v2 on synthetic Rust files.
/// Re-validate if the embedding model changes:
///   θ=0.92 → ~25% of lines must change to drop below
///   θ=0.95 → ~8% of lines must change
///   θ=0.96 → ~4% of lines must change
fn read_dedup_threshold(line_count: usize) -> f32 {
    if line_count > 200 {
        0.96
    } else if line_count > 50 {
        0.95
    } else {
        0.92
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ── Fix 3: infrastructure command skip ───────────────────────────────────────

/// Commands that never produce meaningful output of their own.
/// Any output attributed to them is compound-command stdout leakage.
fn is_no_output_cmd(cmd: &str) -> bool {
    matches!(cmd, "sleep" | "wait" | "true" | "false" | ":")
}

// ── Fix 2: timestamp/counter normalization for polling dedup ─────────────────

/// Fallback used when the BERT budget is exhausted for a given call site.
/// Returns the first HEAD lines and last TAIL lines joined with an omission marker.
/// Never calls BERT. Never panics. Used by the WebFetch handler and future features.
#[allow(dead_code)]
fn bert_budget_fallback(text: &str) -> String {
    const HEAD: usize = 40;
    const TAIL: usize = 20;
    let lines: Vec<&str> = text.lines().collect();
    let n = lines.len();
    if n <= HEAD + TAIL {
        return text.to_string();
    }
    let omitted = n - HEAD - TAIL;
    format!(
        "{}\n[... {} lines omitted — BERT budget exhausted ...]\n{}",
        lines[..HEAD].join("\n"),
        omitted,
        lines[n - TAIL..].join("\n")
    )
}

/// Strip temporal noise (timestamps, counters, UUIDs, git SHAs, IPs) from
/// `text` before computing the embedding for polling suppressor B3 comparison.
/// The output itself is never modified — only the embedding input is normalized.
fn strip_temporal_noise(text: &str) -> String {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(
            r"(?x)
            \d{4}-\d{2}-\d{2}[T\ ]\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:?\d{2})?  # ISO timestamp
            | \d{2}:\d{2}:\d{2}(\.\d+)?          # HH:MM:SS
            | \d{2}:\d{2}                          # HH:MM
            | \b\d{10,13}\b                        # unix timestamp (10-13 digits)
            | \b\d+(\.\d+)?\s*(ms|µs|ns|seconds?|mins?|hours?) # durations
            | \b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b  # UUID
            | \b[0-9a-f]{40}\b                     # git SHA
            | \b0x[0-9a-f]{6,}\b                   # hex address
            | \b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b  # IPv4
            ",
        )
        .expect("strip_temporal_noise regex")
    });
    re.replace_all(text, "").to_string()
}

// ── Cross-file dedup (3.2) ───────────────────────────────────────────────────

/// Split output into paragraph-sized sections and collapse those already seen
/// in recently-read files (cosine ≥ 0.80 against stored section embeddings).
fn apply_cross_file_dedup(
    output: &str,
    session: &mut crate::session::SessionState,
    sid: &str,
) -> String {
    const MIN_SECTION_LINES: usize = 5;
    const SIMILARITY_THRESHOLD: f32 = 0.80;
    const MAX_SUPPRESS_RATIO: f32 = 0.30;

    if session.read_section_embeddings.is_empty() {
        // No prior read sections to compare against — just store and return
        store_section_embeddings(output, session, sid, MIN_SECTION_LINES);
        return output.to_string();
    }

    let sections = segment_output(output, MIN_SECTION_LINES);
    if sections.is_empty() {
        store_section_embeddings(output, session, sid, MIN_SECTION_LINES);
        return output.to_string();
    }

    let texts: Vec<&str> = sections.iter().map(|s| s.as_str()).collect();
    if !crate::bert_budget::try_consume() {
        return output.to_string();
    }
    let embeddings = match panda_core::summarizer::embed_batch(&texts) {
        Ok(e) => e,
        Err(_) => {
            return output.to_string();
        }
    };

    let total_lines = output.lines().count();
    let mut suppressed_lines = 0usize;
    let mut suppress: Vec<bool> = vec![false; sections.len()];

    for (i, emb) in embeddings.iter().enumerate() {
        if session.is_section_seen(emb, SIMILARITY_THRESHOLD) {
            suppress[i] = true;
            suppressed_lines += sections[i].lines().count();
        }
    }

    // Guard: don't suppress more than MAX_SUPPRESS_RATIO
    if suppressed_lines as f32 / total_lines.max(1) as f32 > MAX_SUPPRESS_RATIO {
        // Store all section embeddings for future reads
        session.add_read_section_embeddings(embeddings);
        session.save(sid);
        return output.to_string();
    }

    // Store section embeddings for future reads
    session.add_read_section_embeddings(embeddings);
    session.save(sid);

    if suppress.iter().all(|s| !s) {
        return output.to_string();
    }

    // Rebuild output with suppressed sections collapsed
    let suppressed_map: std::collections::HashMap<&str, bool> = sections
        .iter()
        .zip(suppress.iter())
        .filter(|(_, &s)| s)
        .map(|(sec, _)| (sec.as_str(), true))
        .collect();

    let mut result_lines: Vec<String> = Vec::new();
    let mut current_para: Vec<&str> = Vec::new();

    for line in output.lines().chain(std::iter::once("")) {
        if line.trim().is_empty() {
            if !current_para.is_empty() {
                let para_text = current_para.join("\n");
                if suppressed_map.contains_key(para_text.as_str()) {
                    let n = current_para.len();
                    let zi_id = panda_core::zoom::register(
                        current_para.iter().map(|l| l.to_string()).collect(),
                    );
                    result_lines.push(format!(
                        "[{} lines: already read — panda expand {}]",
                        n, zi_id
                    ));
                } else {
                    result_lines.extend(current_para.iter().map(|l| l.to_string()));
                    result_lines.push(String::new());
                }
                current_para.clear();
            }
        } else {
            current_para.push(line);
        }
    }

    if result_lines.last().map_or(false, |l| l.is_empty()) {
        result_lines.pop();
    }

    result_lines.join("\n")
}

fn store_section_embeddings(
    output: &str,
    session: &mut crate::session::SessionState,
    sid: &str,
    min_lines: usize,
) {
    let sections = segment_output(output, min_lines);
    if sections.is_empty() {
        return;
    }
    let texts: Vec<&str> = sections.iter().map(|s| s.as_str()).collect();
    if crate::bert_budget::try_consume() {
        if let Ok(embeddings) = panda_core::summarizer::embed_batch(&texts) {
            session.add_read_section_embeddings(embeddings);
            session.save(sid);
        }
    }
}

// ── Edit-aware compression (3.3) ────────────────────────────────────────────

/// Preserve context around recently-edited lines, compress everything else more
/// aggressively. `preserve_ranges` contains (start, end) line ranges to keep.
fn apply_edit_aware_compression(output: &str, preserve_ranges: &[(usize, usize)]) -> String {
    let lines: Vec<&str> = output.lines().collect();
    let n = lines.len();
    if n == 0 || preserve_ranges.is_empty() {
        return output.to_string();
    }

    // Build a set of lines that should be preserved
    let mut preserve = vec![false; n];
    for &(start, end) in preserve_ranges {
        for i in start..end.min(n) {
            preserve[i] = true;
        }
    }

    let mut result: Vec<String> = Vec::new();
    let mut skipped = 0usize;

    for (i, line) in lines.iter().enumerate() {
        if preserve[i] {
            if skipped > 0 {
                let zi_id = panda_core::zoom::register(
                    lines[i.saturating_sub(skipped)..i]
                        .iter()
                        .map(|l| l.to_string())
                        .collect(),
                );
                result.push(format!(
                    "[{} lines compressed — panda expand {}]",
                    skipped, zi_id
                ));
                skipped = 0;
            }
            result.push(line.to_string());
        } else {
            skipped += 1;
        }
    }

    if skipped > 0 {
        let start = n.saturating_sub(skipped);
        let zi_id = panda_core::zoom::register(
            lines[start..n].iter().map(|l| l.to_string()).collect(),
        );
        result.push(format!(
            "[{} lines compressed — panda expand {}]",
            skipped, zi_id
        ));
    }

    result.join("\n")
}

// ── Focus graph embedding refresh (2.3) ──────────────────────────────────────

/// After reading a file, re-embed it and update the focus graph so that
/// the most frequently read files always have the freshest embeddings.
fn refresh_focus_embedding(file_path: &str) {
    // Resolve the relative path (strip repo root prefix)
    let rel_path = std::env::current_dir()
        .ok()
        .and_then(|cwd| {
            std::path::Path::new(file_path)
                .strip_prefix(&cwd)
                .ok()
                .map(|p| p.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| file_path.to_string());

    // Find the focus graph DB for the current repo
    let repo_root = match std::env::current_dir() {
        Ok(d) => d,
        Err(_) => return,
    };
    let repo_hash = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        repo_root.to_string_lossy().hash(&mut hasher);
        format!("{:x}", hasher.finish())
    };
    let Some(home) = dirs::home_dir() else { return };
    let index_parent = home.join(".local/share/panda/indexes").join(&repo_hash);

    let head = match panda_core::focus::indexer::current_head(&repo_root) {
        Ok(h) => h,
        Err(_) => return,
    };
    let db_path = index_parent.join(&head).join("graph.sqlite");
    if !db_path.exists() {
        return;
    }

    // Re-embed the file content
    let content = match std::fs::read_to_string(file_path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let content_prefix: String = content.chars().take(1000).collect();
    let embed_text = format!("{}\n{}", rel_path, content_prefix);
    if !crate::bert_budget::try_consume() {
        return;
    }
    let embeddings = match panda_core::summarizer::embed_batch(&[embed_text.as_str()]) {
        Ok(e) => e,
        Err(_) => return,
    };
    let Some(emb) = embeddings.first() else { return };
    let blob: Vec<u8> = emb.iter().flat_map(|f| f.to_le_bytes()).collect();

    // Open the graph read-write and update
    if let Ok(conn) = rusqlite::Connection::open(&db_path) {
        let _ = panda_core::focus::update_embedding(&conn, &rel_path, &blob);
    }
}

// ── Adaptive pressure helper (Feature 2) ─────────────────────────────────────

/// Write raw command output to a tee file for recovery when compression is aggressive.
/// Returns the path if successful.
fn write_hook_tee(cmd: &str, content: &str) -> Option<std::path::PathBuf> {
    // Only tee if content is substantial (>1KB)
    if content.len() < 1024 {
        return None;
    }
    let tee_dir = dirs::data_local_dir()?.join("panda").join("tee");
    std::fs::create_dir_all(&tee_dir).ok()?;

    // Auto-rotate: keep max 50 tee files (synchronous — fast readdir, no I/O wait)
    if let Ok(entries) = std::fs::read_dir(&tee_dir) {
        let mut files: Vec<std::path::PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("log"))
            .collect();
        if files.len() > 50 {
            files.sort();
            for old in files.iter().take(files.len() - 50) {
                let _ = std::fs::remove_file(old);
            }
        }
    }

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let safe_cmd: String = cmd
        .chars()
        .take(30)
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let path = tee_dir.join(format!("{}_{}.log", ts, safe_cmd));

    // Write the tee file on a background thread — the caller only needs the path
    // (for the "[full output: ...]" hint), not the write to complete.
    let path_bg = path.clone();
    let content_bg = content.to_string();
    std::thread::spawn(move || { let _ = std::fs::write(&path_bg, content_bg); });

    Some(path)
}

/// Reorder output lines to surface lines mentioning focused files first.
///
/// After the handler compresses output, diagnostic lines about files the agent
/// is actively editing/reading are most valuable — they should appear before
/// noise from unrelated files. This function:
///   1. Partitions lines into "focused" (mention a focused filename) vs "other".
///   2. Returns focused lines first, then up to `cap` other lines.
///   3. Appends a summary if other lines were truncated.
///
/// No-ops when: clean_room mode, no focused files in session, or no focused
/// lines appear in output (avoids reordering unrelated output).
fn apply_focus_boost(
    output: &str,
    session: &crate::session::SessionState,
    clean_room: bool,
) -> String {
    if clean_room {
        return output.to_string();
    }
    let focused = session.focused_file_paths();
    if focused.is_empty() {
        return output.to_string();
    }

    let mut focused_lines: Vec<&str> = Vec::new();
    let mut other_lines: Vec<&str> = Vec::new();

    for line in output.lines() {
        if focused.iter().any(|f| line.contains(f.as_str())) {
            focused_lines.push(line);
        } else {
            other_lines.push(line);
        }
    }

    // If nothing matches focus, pass through unchanged — don't reorder unrelated output.
    if focused_lines.is_empty() {
        return output.to_string();
    }

    // Cap how many "other" lines we include so focused content stays prominent.
    const OTHER_CAP: usize = 30;
    let cap = OTHER_CAP.saturating_sub(focused_lines.len());
    let extra = other_lines.len().saturating_sub(cap);

    let mut result = focused_lines.join("\n");
    if !other_lines.is_empty() {
        let shown: Vec<&str> = other_lines.iter().copied().take(cap).collect();
        if !shown.is_empty() {
            result.push('\n');
            result.push_str(&shown.join("\n"));
        }
        if extra > 0 {
            result.push_str(&format!("\n[+{} lines from non-focused files]", extra));
        }
    }
    result
}

/// Convert centroid similarity to an additive pressure modifier.
/// Similarity < 0.85 → 0 (novel content, no extra pressure).
/// Similarity 0.85→1.0 → linearly maps to 0.0→0.32.
fn stability_to_pressure(sim: f32) -> f32 {
    if sim < 0.85 {
        return 0.0;
    }
    ((sim - 0.85) / 0.15 * 0.8).min(0.8) * 0.4
}

// ── Zoom label enrichment (Feature 3) ────────────────────────────────────────

/// Infer a human-readable category for a collapsed block from its lines.
fn categorize_block(lines: &[String]) -> &'static str {
    if lines.is_empty() {
        return "filtered output";
    }
    // Build/download progress: all lines start with well-known prefixes
    if lines.iter().all(|l| {
        let l = l.trim_start();
        l.starts_with("Compiling")
            || l.starts_with("Checking")
            || l.starts_with("Download")
            || l.starts_with("Fetching")
            || l.starts_with("Updating")
    }) {
        return "build/download progress";
    }

    let joined = lines.iter().map(|l| l.as_str()).collect::<Vec<_>>().join("\n");
    let low = joined.to_lowercase();

    if low.contains("error:") || low.contains("error[") {
        return "error details";
    }
    if low.contains("panicked") || low.contains("stack backtrace") {
        return "stack trace";
    }
    if (low.contains("passed") || low.contains(" ok")) && !low.contains("failed") && !low.contains("error:") {
        if low.contains("test") {
            return "passing tests";
        }
    }
    if low.contains("warning:") {
        return "compiler warnings";
    }
    if low.contains("note:") {
        return "compiler notes";
    }
    if lines.iter().all(|l| l.starts_with("  ") || l.starts_with('\t')) {
        return "indented detail";
    }
    "filtered output"
}

/// Replace generic zoom markers like `[20 matching lines collapsed — ccr expand ZI_1]`
/// with descriptive ones like `[20 lines: compiler warnings — ccr expand ZI_1]`.
fn enrich_zoom_labels(output: &str, zoom_blocks: &[panda_core::zoom::ZoomBlock]) -> String {
    let mut result = output.to_string();
    for block in zoom_blocks {
        let label = categorize_block(&block.lines);
        let expand_tag = format!("panda expand {}", block.id);
        result = result
            .lines()
            .map(|line| {
                if line.contains(&expand_tag) {
                    format!("[{} lines: {} — {}]", block.lines.len(), label, expand_tag)
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    result
}

// ── Cross-command semantic deduplication (Feature 4) ─────────────────────────

/// Split `text` into paragraph-sized segments (groups of non-blank lines separated
/// by blank lines). Only segments with at least `min_lines` lines are returned.
fn segment_output(text: &str, min_lines: usize) -> Vec<String> {
    let mut segments: Vec<String> = Vec::new();
    let mut current: Vec<&str> = Vec::new();

    for line in text.lines() {
        if line.trim().is_empty() {
            if current.len() >= min_lines {
                segments.push(current.join("\n"));
            }
            current.clear();
        } else {
            current.push(line);
        }
    }
    if current.len() >= min_lines {
        segments.push(current.join("\n"));
    }
    segments
}

/// Rebuild `original` output, replacing suppressed segments with zoom block markers.
/// `suppress[i] = Some(turn)` means segment `i` was seen in `turn` of a different command.
fn rebuild_with_suppressions(
    original: &str,
    segments: &[String],
    suppress: &[Option<usize>],
) -> String {
    // Build a lookup from segment text → referenced turn
    let suppressed: std::collections::HashMap<&str, usize> = segments
        .iter()
        .zip(suppress.iter())
        .filter_map(|(seg, s)| s.map(|turn| (seg.as_str(), turn)))
        .collect();

    if suppressed.is_empty() {
        return original.to_string();
    }

    let mut output_lines: Vec<String> = Vec::new();
    let mut current_para: Vec<&str> = Vec::new();
    // Chain a sentinel blank line so the last paragraph is always flushed.
    let lines_with_sentinel: Vec<&str> = original.lines().chain(std::iter::once("")).collect();

    for line in lines_with_sentinel {
        if line.trim().is_empty() {
            if !current_para.is_empty() {
                let para_text = current_para.join("\n");
                if let Some(&turn) = suppressed.get(para_text.as_str()) {
                    let n = current_para.len();
                    // Register as a zoom block so `panda expand ZI_N` works.
                    let zi_id = panda_core::zoom::register(
                        current_para.iter().map(|l| l.to_string()).collect(),
                    );
                    output_lines.push(format!(
                        "[{} lines: already shown (turn {}) — panda expand {}]",
                        n, turn, zi_id
                    ));
                } else {
                    output_lines.extend(current_para.iter().map(|l| l.to_string()));
                    output_lines.push(String::new());
                }
                current_para.clear();
            }
        } else {
            current_para.push(line);
        }
    }

    // Strip trailing blank sentinel if present
    if output_lines.last().map_or(false, |l| l.is_empty()) {
        output_lines.pop();
    }

    output_lines.join("\n")
}

/// Suppress segments of the current output that are semantically similar
/// (cosine ≥ 0.85) to segments from the last 5 entries of *different* commands.
/// Replaces suppressed segments with zoom block back-reference markers.
///
/// Guards: only runs when output ≥ 20 lines, skips state commands, and never
/// suppresses more than 40% of total output lines.
fn cross_command_dedup(
    output: &str,
    cmd_key: &str,
    session: &crate::session::SessionState,
    sid: &str,
    is_state: bool,
) -> String {
    const MIN_LINES: usize = 20;
    const MIN_SEGMENT_LINES: usize = 5;
    const SIMILARITY_THRESHOLD: f32 = 0.85;
    const MAX_SUPPRESS_RATIO: f32 = 0.40;

    if is_state {
        return output.to_string();
    }
    let total_lines = output.lines().count();
    if total_lines < MIN_LINES {
        return output.to_string();
    }

    let current_segments = segment_output(output, MIN_SEGMENT_LINES);
    if current_segments.is_empty() {
        return output.to_string();
    }

    // Gather up to 5 entries from different commands (most recent first)
    let prior_entries: Vec<&crate::session::SessionEntry> = session
        .entries
        .iter()
        .rev()
        .filter(|e| e.cmd != cmd_key)
        .take(5)
        .collect();
    if prior_entries.is_empty() {
        return output.to_string();
    }

    // Build (turn, segment_text) pairs from prior entries
    let prior_segments: Vec<(usize, String)> = prior_entries
        .iter()
        .flat_map(|e| {
            let content = e.state_content.as_deref().unwrap_or(&e.content_preview);
            segment_output(content, MIN_SEGMENT_LINES)
                .into_iter()
                .map(|s| (e.turn, s))
        })
        .collect();

    if prior_segments.is_empty() {
        return output.to_string();
    }

    // Batch-embed all segments in one call
    let all_texts: Vec<&str> = current_segments
        .iter()
        .map(|s| s.as_str())
        .chain(prior_segments.iter().map(|(_, s)| s.as_str()))
        .collect();

    if !crate::bert_budget::try_consume() {
        return output.to_string();
    }
    let embeddings = match panda_core::summarizer::embed_batch(&all_texts) {
        Ok(e) => e,
        Err(_) => return output.to_string(),
    };

    let n_cur = current_segments.len();
    let cur_embs = &embeddings[..n_cur];
    let prior_embs = &embeddings[n_cur..];

    // For each current segment, find the most similar prior segment
    let mut suppress: Vec<Option<usize>> = vec![None; n_cur];
    let mut suppressed_lines = 0usize;

    for (i, cur_emb) in cur_embs.iter().enumerate() {
        for (j, prior_emb) in prior_embs.iter().enumerate() {
            let sim = crate::handlers::util::cosine_similarity(cur_emb, prior_emb);
            if sim >= SIMILARITY_THRESHOLD {
                suppress[i] = Some(prior_segments[j].0);
                suppressed_lines += current_segments[i].lines().count();
                break;
            }
        }
    }

    // Guard: don't suppress more than MAX_SUPPRESS_RATIO of all lines
    if suppressed_lines as f32 / total_lines as f32 > MAX_SUPPRESS_RATIO {
        return output.to_string();
    }

    if suppress.iter().all(|s| s.is_none()) {
        return output.to_string();
    }

    let _ = sid; // session_id reserved for future use (e.g. incremental zoom block saving)
    rebuild_with_suppressions(output, &current_segments, &suppress)
}

// ── WebFetch handler ──────────────────────────────────────────────────────────

/// Compress fetched web pages: collapse boilerplate sections, BERT-summarize
/// content sections up to the remaining BERT budget, protect code blocks.
fn process_webfetch(hook_input: HookInput) -> Result<Option<String>> {
    const SHORT_LINE_THRESHOLD: usize = 30;

    let url = hook_input
        .tool_input
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let output_text = if !hook_input.tool_response.output.is_empty() {
        hook_input.tool_response.output.clone()
    } else {
        hook_input.tool_response.stdout.clone()
    };

    if output_text.is_empty() {
        return Ok(None);
    }

    let line_count = output_text.lines().count();
    if line_count < SHORT_LINE_THRESHOLD {
        return Ok(None); // short page — pass through unchanged
    }

    // URL dedup: same URL fetched earlier this session → return marker
    let sid = crate::session::session_id();
    let mut session = crate::session::SessionState::load(&sid);
    let url_key = format!("(webfetch) {}", url);
    if let Some(hit) = session.find_similar(&url_key, &[]) {
        // find_similar won't match on empty embedding — use find_exact instead
        let _ = hit;
    }
    // Exact URL dedup via session content_preview
    if let Some(hit) = session.find_exact(&url_key, &output_text.chars().take(4000).collect::<String>()) {
        let age = crate::session::format_age(hit.age_secs);
        return Ok(Some(serde_json::to_string(&HookOutput {
            output: format!(
                "[same WebFetch content as turn {} ({} ago) — {} tokens saved]",
                hit.turn, age, hit.tokens_saved
            ),
        })?));
    }

    // Data formats (JSON, XML, HTML, code) must never be compressed — structure
    // is semantically load-bearing and BERT summarization would mangle it.
    if is_passthrough_web_content(&output_text) {
        return Ok(None);
    }

    // Split into sections and score by keyword density (no BERT cost)
    let sections = split_web_sections(&output_text);
    if sections.is_empty() {
        return Ok(None);
    }
    let mut scored: Vec<(usize, u32, &str)> = sections
        .iter()
        .enumerate()
        .map(|(idx, text)| (idx, score_web_section(text), text.as_str()))
        .collect();
    // Sort by score descending so highest-value sections get BERT first
    scored.sort_by(|a, b| b.1.cmp(&a.1));

    panda_core::zoom::enable();
    let mut processed: Vec<(usize, String)> = Vec::with_capacity(sections.len());

    for (orig_idx, _score, section_text) in &scored {
        // Reserve 2 BERT calls for downstream cross-command dedup + centroid
        if crate::bert_budget::remaining() > 2 && crate::bert_budget::try_consume() {
            let summarized = summarize_web_section(section_text);
            processed.push((*orig_idx, summarized));
        } else {
            processed.push((*orig_idx, collapse_section_simple(section_text)));
        }
    }

    // Drain and save any zoom blocks registered during summarization
    let web_blocks = panda_core::zoom::drain();
    if !web_blocks.is_empty() {
        let _ = crate::zoom_store::save_blocks(&sid, web_blocks, Some("(webfetch)"));
    }

    // Reconstruct in original document order
    processed.sort_by_key(|(idx, _)| *idx);
    let final_output = processed
        .into_iter()
        .map(|(_, text)| text)
        .collect::<Vec<_>>()
        .join("\n\n");

    // Record URL for dedup on subsequent fetches
    if crate::bert_budget::try_consume() {
        if let Ok(mut embs) = panda_core::summarizer::embed_batch(&[final_output.as_str()]) {
            if let Some(emb) = embs.pop() {
                let tokens = panda_core::tokens::count_tokens(&final_output);
                session.record(&url_key, emb, tokens, &final_output, false, None);
                session.save(&sid);
            }
        }
    }

    let in_tok = panda_core::tokens::count_tokens(&output_text);
    let out_tok = panda_core::tokens::count_tokens(&final_output);

    // Use raw-content token counts (before any notice is prepended) so the
    // percentage reflects actual content reduction, not notice overhead.
    let saved_pct = if in_tok > out_tok {
        (in_tok - out_tok) * 100 / in_tok
    } else {
        0
    };

    // Collect zoom IDs present in the output so Claude knows which collapsed
    // sections are recoverable and by what ID.
    let zoom_ids: Vec<String> = {
        let mut ids = Vec::new();
        let mut rest = final_output.as_str();
        while let Some(pos) = rest.find("ZI_") {
            let after = &rest[pos..];
            let end = after
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(after.len());
            ids.push(after[..end].to_string());
            rest = &after[end..];
        }
        ids.dedup();
        ids
    };

    // Skip the notice when compression made no meaningful difference.
    // Avoids confusing "~0% compressed" on pages where markers added bytes.
    if saved_pct == 0 && zoom_ids.is_empty() {
        crate::util::append_analytics(&panda_core::analytics::Analytics::new(
            in_tok, out_tok, Some("(webfetch)".to_string()), None, None,
        ));
        return Ok(Some(serde_json::to_string(&HookOutput { output: final_output })?));
    }

    let notice = if zoom_ids.is_empty() {
        format!(
            "[PandaFilter: WebFetch compressed ~{}% — content summarised. \
             If you need more detail, re-fetch the URL: {}]",
            saved_pct, url
        )
    } else {
        format!(
            "[PandaFilter: WebFetch compressed ~{}% — content summarised. \
             Collapsed sections: {}. \
             Run `panda expand <ID>` for any section, or re-fetch {} for the full page.]",
            saved_pct,
            zoom_ids.join(", "),
            url
        )
    };

    let output_with_notice = format!("{}\n\n{}", notice, final_output);

    crate::util::append_analytics(&panda_core::analytics::Analytics::new(
        in_tok,
        out_tok,
        Some("(webfetch)".to_string()),
        None,
        None,
    ));

    Ok(Some(serde_json::to_string(&HookOutput { output: output_with_notice })?))
}

/// Returns true for content that must not be compressed — BERT summarization
/// would destroy the semantic structure of data formats and code.
fn is_passthrough_web_content(text: &str) -> bool {
    // Inspect the first non-empty line for leading format indicators.
    let first = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim())
        .unwrap_or("");

    // JSON / YAML front-matter / XML / HTML / shell
    if first.starts_with('{')
        || first.starts_with('[')
        || first.starts_with("<?")
        || first.starts_with("<!DOCTYPE")
        || first.starts_with("<!--")
        || first.starts_with("#!/")
    {
        return true;
    }

    // Heavily tagged HTML that wasn't stripped (> 20% of lines start with `<`)
    let total = text.lines().count();
    if total > 10 {
        let tag_lines = text
            .lines()
            .filter(|l| {
                let t = l.trim();
                t.starts_with('<') && (t.ends_with('>') || t.contains("</"))
            })
            .count();
        if tag_lines * 100 / total > 20 {
            return true;
        }
    }

    false
}

/// Split a web page into logical sections using a cascading strategy so the
/// function works on all real-world content formats, not just Markdown docs.
///
/// Strategy (applied in order, stops at the first that yields ≥ 3 sections):
///   1. Any Markdown/AsciiDoc heading (`#`/`##`/`###`) or 2+ blank lines
///      → best for structured documentation pages
///   2. Single blank lines → best for prose/blog pages
///   3. Fixed 40-line chunks → last-resort for dense continuous text
///
/// Sections larger than MAX_SECTION_LINES are split into 40-line sub-sections
/// so no single BERT call has to process a wall of text.
fn split_web_sections(text: &str) -> Vec<String> {
    const MAX_SECTION_LINES: usize = 60;
    const CHUNK_SIZE: usize = 40;

    // ── Strategy 1: headers + double blank lines ─────────────────────────────
    let s1 = split_on_headers_or_double_blanks(text);
    let sections = if s1.len() >= 3 {
        s1
    } else {
        // ── Strategy 2: single blank lines (paragraph boundaries) ────────────
        let s2 = split_on_single_blanks(text);
        if s2.len() >= 3 {
            s2
        } else {
            // ── Strategy 3: fixed chunks ──────────────────────────────────────
            text.lines()
                .collect::<Vec<_>>()
                .chunks(CHUNK_SIZE)
                .map(|chunk| chunk.join("\n"))
                .filter(|s| !s.trim().is_empty())
                .collect()
        }
    };

    // Sub-divide any section that is still too large so each BERT call stays cheap.
    sections
        .into_iter()
        .flat_map(|section| {
            let line_count = section.lines().count();
            if line_count > MAX_SECTION_LINES {
                section
                    .lines()
                    .collect::<Vec<_>>()
                    .chunks(CHUNK_SIZE)
                    .map(|chunk| chunk.join("\n"))
                    .filter(|s| !s.trim().is_empty())
                    .collect::<Vec<_>>()
            } else {
                vec![section]
            }
        })
        .filter(|s| !s.trim().is_empty())
        .collect()
}

/// Split on Markdown/AsciiDoc headings (`#`, `##`, `###`) or two or more
/// consecutive blank lines. Used as the primary split strategy.
fn split_on_headers_or_double_blanks(text: &str) -> Vec<String> {
    let mut sections: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut blank_run = 0usize;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            blank_run += 1;
            if blank_run >= 2 && !current.trim().is_empty() {
                sections.push(current.trim().to_string());
                current = String::new();
            } else {
                current.push('\n');
            }
        } else if trimmed.starts_with('#') && !current.trim().is_empty() {
            // Any Markdown heading level (#, ##, ###, ####)
            sections.push(current.trim().to_string());
            current = format!("{}\n", line);
            blank_run = 0;
        } else {
            blank_run = 0;
            current.push_str(line);
            current.push('\n');
        }
    }
    if !current.trim().is_empty() {
        sections.push(current.trim().to_string());
    }
    sections.into_iter().filter(|s| !s.trim().is_empty()).collect()
}

/// Split on single blank lines — used for prose/blog content where paragraphs
/// are separated by a single blank line rather than headings.
fn split_on_single_blanks(text: &str) -> Vec<String> {
    let mut sections: Vec<String> = Vec::new();
    let mut current = String::new();

    for line in text.lines() {
        if line.trim().is_empty() {
            if !current.trim().is_empty() {
                sections.push(current.trim().to_string());
                current = String::new();
            }
        } else {
            current.push_str(line);
            current.push('\n');
        }
    }
    if !current.trim().is_empty() {
        sections.push(current.trim().to_string());
    }
    sections.into_iter().filter(|s| !s.trim().is_empty()).collect()
}

/// Score a section by keyword density — higher = more information-dense.
/// Used to prioritise which sections get BERT summarization when budget is limited.
fn score_web_section(section: &str) -> u32 {
    const KEYWORDS: &[&str] = &[
        "error", "warning", "usage", "example", "note", "tip", "return",
        "parameter", "argument", "important", "required", "optional", "type",
        "function", "method", "struct", "interface", "class", "const", "fn ",
        "pub ", "impl ", "```",
    ];
    // Navigation/footer patterns that score negatively
    const NOISE: &[&str] = &[
        "skip to", "copyright", "terms of", "privacy policy", "cookie",
        "subscribe", "newsletter", "follow us", "share this",
    ];

    let lower = section.to_lowercase();
    let pos: u32 = KEYWORDS.iter().map(|k| lower.matches(k).count() as u32).sum();
    let neg: u32 = NOISE.iter().map(|k| lower.matches(k).count() as u32).sum();
    pos.saturating_sub(neg * 3)
}

/// Collapse a section to its header + first 3 non-empty content lines.
/// When lines are omitted a zoom block is registered so Claude can expand
/// the full section with `panda expand ZI_N`. Used when BERT budget is exhausted.
fn collapse_section_simple(section: &str) -> String {
    let lines: Vec<&str> = section.lines().collect();
    let header = lines.first().copied().unwrap_or("").to_string();
    let body: Vec<&str> = lines
        .iter()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .take(3)
        .copied()
        .collect();
    if body.is_empty() {
        return header;
    }
    let n_remaining = lines.iter().skip(1).filter(|l| !l.trim().is_empty()).count();
    let omitted = n_remaining.saturating_sub(body.len());
    if omitted > 0 {
        // Register omitted lines as a zoom block so the content is recoverable.
        let omitted_lines: Vec<String> = lines
            .iter()
            .skip(1)
            .filter(|l| !l.trim().is_empty())
            .skip(3)
            .map(|l| l.to_string())
            .collect();
        let zi_id = panda_core::zoom::register(omitted_lines);
        format!(
            "{}\n{}\n[... {} lines — panda expand {}]",
            header,
            body.join("\n"),
            omitted,
            zi_id
        )
    } else {
        format!("{}\n{}", header, body.join("\n"))
    }
}

/// BERT-summarize a single web section using the existing line-level summarizer.
fn summarize_web_section(section: &str) -> String {
    let line_count = section.lines().count();
    if line_count <= 5 {
        return section.to_string();
    }
    // Protect code blocks — never compress their content
    let has_code_block = section.contains("```");
    if has_code_block {
        // Only summarize non-code-block portions; return the whole section if
        // detecting fences would be complex. Safety: preserve content intact.
        return section.to_string();
    }
    let budget = (line_count / 3).max(3).min(20);
    panda_core::summarizer::summarize(section, budget).output
}

// ── WebSearch handler ─────────────────────────────────────────────────────────

/// Compress WebSearch results: rank by intent relevance, keep top 8,
/// collapse lower-ranked results into a zoom block.
fn process_websearch(hook_input: HookInput) -> Result<Option<String>> {
    const KEEP_RESULTS: usize = 8;
    const MIN_RESULTS_TO_PROCESS: usize = 8;

    let output_text = if !hook_input.tool_response.output.is_empty() {
        hook_input.tool_response.output.clone()
    } else {
        hook_input.tool_response.stdout.clone()
    };

    if output_text.is_empty() {
        return Ok(None);
    }

    let results = parse_search_results(&output_text);
    if results.len() <= MIN_RESULTS_TO_PROCESS {
        return Ok(None); // few results — pass through unchanged
    }

    // Score by intent similarity when BERT budget allows; fall back to order
    let query = crate::intent::extract_intent_multi(3);
    let ranked = if let Some(ref q) = query {
        if crate::bert_budget::try_consume() {
            rank_search_results_by_intent(&results, q)
        } else {
            (0..results.len()).collect()
        }
    } else {
        (0..results.len()).collect()
    };

    // Always keep at least MIN_RESULTS_TO_PROCESS, cap at KEEP_RESULTS
    let keep_count = KEEP_RESULTS.min(ranked.len());
    let keep_indices: std::collections::HashSet<usize> =
        ranked[..keep_count].iter().copied().collect();

    let kept: Vec<&str> = ranked[..keep_count]
        .iter()
        .map(|&i| results[i].as_str())
        .collect();
    let collapsed: Vec<String> = (0..results.len())
        .filter(|i| !keep_indices.contains(i))
        .map(|i| results[i].clone())
        .collect();

    panda_core::zoom::enable();
    let mut final_output = kept.join("\n\n");

    if !collapsed.is_empty() {
        let n = collapsed.len();
        let zi_id = panda_core::zoom::register(collapsed);
        final_output.push_str(&format!(
            "\n\n[{} more results — panda expand {}]",
            n, zi_id
        ));
    }

    let sid = crate::session::session_id();
    let web_blocks = panda_core::zoom::drain();
    if !web_blocks.is_empty() {
        let _ = crate::zoom_store::save_blocks(&sid, web_blocks, Some("(websearch)"));
    }

    let in_tok = panda_core::tokens::count_tokens(&output_text);
    let out_tok = panda_core::tokens::count_tokens(&final_output);
    crate::util::append_analytics(&panda_core::analytics::Analytics::new(
        in_tok,
        out_tok,
        Some("(websearch)".to_string()),
        None,
        None,
    ));

    Ok(Some(serde_json::to_string(&HookOutput { output: final_output })?))
}

/// Parse search results from markdown-list formatted WebSearch output.
/// Each result is a markdown list item block starting with `- ` or `* `.
fn parse_search_results(text: &str) -> Vec<String> {
    let mut results = Vec::new();
    let mut current = String::new();

    for line in text.lines() {
        if (line.starts_with("- ") || line.starts_with("* ")) && !current.trim().is_empty() {
            results.push(current.trim().to_string());
            current = format!("{}\n", line);
        } else if line.starts_with("- ") || line.starts_with("* ") {
            current = format!("{}\n", line);
        } else if !current.is_empty() {
            current.push_str(line);
            current.push('\n');
        }
    }
    if !current.trim().is_empty() {
        results.push(current.trim().to_string());
    }
    results
}

/// Rank search results by cosine similarity to the intent embedding.
/// Returns result indices sorted by relevance descending.
fn rank_search_results_by_intent(results: &[String], intent: &str) -> Vec<usize> {
    let mut texts: Vec<&str> = results.iter().map(|s| s.as_str()).collect();
    texts.push(intent);

    let embeddings = match panda_core::summarizer::embed_batch(&texts) {
        Ok(e) => e,
        Err(_) => return (0..results.len()).collect(),
    };

    let intent_emb = &embeddings[embeddings.len() - 1];
    let result_embs = &embeddings[..results.len()];

    let mut scored: Vec<(usize, f32)> = result_embs
        .iter()
        .enumerate()
        .map(|(i, emb)| {
            let sim = emb.iter().zip(intent_emb.iter()).map(|(a, b)| a * b).sum::<f32>();
            (i, sim)
        })
        .collect();

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().map(|(i, _)| i).collect()
}

// ── Lifecycle hook handlers ───────────────────────────────────────────────────

fn process_compact_capture() -> Result<()> {
    let sid = crate::session::session_id();
    let session = crate::session::SessionState::load(&sid);

    if session.total_turns == 0 {
        // Nothing worth capturing
        print!("{{\"suppressOutput\": true}}");
        return Ok(());
    }

    let digest = session.extract_digest();

    // SHA-256 fingerprint: session_id + top-5 edited file paths
    let fp_input = {
        let mut s = sid.clone();
        let mut edits: Vec<&str> = session.recent_edits.keys().map(|k| k.as_str()).collect();
        edits.sort();
        edits.truncate(5);
        for p in edits {
            s.push('|');
            s.push_str(p);
        }
        s
    };
    let fingerprint = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        fp_input.hash(&mut h);
        format!("{:x}", h.finish())
    };

    // Locate compacts directory
    let compact_dir = {
        let base = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        std::path::PathBuf::from(base)
            .join(".local")
            .join("share")
            .join("panda")
            .join("compacts")
    };
    std::fs::create_dir_all(&compact_dir).ok();

    // Deduplication: check most recent compact for this session
    if let Ok(entries) = std::fs::read_dir(&compact_dir) {
        let prefix = format!("{}-", sid);
        let mut matches: Vec<std::path::PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with(&prefix) && n.ends_with(".md"))
                    .unwrap_or(false)
            })
            .collect();
        matches.sort();
        if let Some(last) = matches.last() {
            if let Ok(content) = std::fs::read_to_string(last) {
                let fp_line = format!("<!-- fingerprint: {} -->", fingerprint);
                if content.contains(&fp_line) {
                    print!("{{\"suppressOutput\": true}}");
                    return Ok(());
                }
            }
        }
    }

    // Write the compact digest
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let filename = compact_dir.join(format!("{}-{}.md", sid, ts));
    let content = format!("<!-- fingerprint: {} -->\n{}", fingerprint, digest.markdown);
    std::fs::write(&filename, &content)
        .unwrap_or_else(|e| eprintln!("[panda] compact capture write error: {}", e));

    print!("{{\"suppressOutput\": true}}");
    Ok(())
}

fn process_compact_restore() -> Result<()> {
    let sid = crate::session::session_id();

    let compact_dir = {
        let base = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        std::path::PathBuf::from(base)
            .join(".local")
            .join("share")
            .join("panda")
            .join("compacts")
    };

    let Ok(entries) = std::fs::read_dir(&compact_dir) else {
        return Ok(());
    };

    let prefix = format!("{}-", sid);
    let mut matches: Vec<std::path::PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with(&prefix) && n.ends_with(".md"))
                .unwrap_or(false)
        })
        .collect();

    if matches.is_empty() {
        return Ok(());
    }

    matches.sort();
    let latest = matches.last().unwrap();

    let content = match std::fs::read_to_string(latest) {
        Ok(c) => c,
        Err(_) => return Ok(()),
    };

    // Strip the fingerprint comment line
    let body: String = content
        .lines()
        .filter(|l| !l.starts_with("<!-- fingerprint:"))
        .collect::<Vec<&str>>()
        .join("\n");

    let response = serde_json::json!({ "additionalContext": body });
    print!("{}", response);
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grep_line_path_extracts_file_prefix() {
        assert_eq!(grep_line_path("src/main.rs:42: fn main() {"), Some("src/main.rs"));
        assert_eq!(grep_line_path("lib/utils.py:def helper():"), Some("lib/utils.py"));
        // No path-like prefix
        assert_eq!(grep_line_path("just some text: with colon"), None);
        assert_eq!(grep_line_path(""), None);
        // Bare word without slash or dot is not a path
        assert_eq!(grep_line_path("warning: something"), None);
    }

    #[test]
    fn grep_line_content_strips_path_and_lineno() {
        assert_eq!(grep_line_content("src/main.rs:42:fn main() {"), "fn main() {");
        assert_eq!(grep_line_content("src/main.rs:fn main() {"), "fn main() {");
        // Content colons survive
        assert_eq!(
            grep_line_content("a/b.go:7:x := map[string]int{}"),
            "x := map[string]int{}"
        );
        // Lines without path prefix unchanged
        assert_eq!(grep_line_content("plain text line"), "plain text line");
    }

    #[test]
    fn read_dedup_threshold_scales_with_size() {
        // Small files: most lenient
        assert_eq!(read_dedup_threshold(10), 0.92);
        assert_eq!(read_dedup_threshold(50), 0.92);
        // Medium files: stricter
        assert_eq!(read_dedup_threshold(51), 0.95);
        assert_eq!(read_dedup_threshold(200), 0.95);
        // Large files: strictest
        assert_eq!(read_dedup_threshold(201), 0.96);
        assert_eq!(read_dedup_threshold(5000), 0.96);
    }

    #[test]
    fn read_dedup_threshold_zero_lines() {
        assert_eq!(read_dedup_threshold(0), 0.92);
    }

    #[test]
    fn read_dedup_threshold_monotonically_increases() {
        let small = read_dedup_threshold(30);
        let medium = read_dedup_threshold(100);
        let large = read_dedup_threshold(500);
        assert!(small <= medium);
        assert!(medium <= large);
    }

    #[test]
    fn is_no_output_cmd_matches_infrastructure() {
        assert!(is_no_output_cmd("sleep"));
        assert!(is_no_output_cmd("wait"));
        assert!(is_no_output_cmd("true"));
        assert!(is_no_output_cmd("false"));
        assert!(is_no_output_cmd(":"));
    }

    #[test]
    fn is_no_output_cmd_does_not_match_real_commands() {
        assert!(!is_no_output_cmd("cargo"));
        assert!(!is_no_output_cmd("git"));
        assert!(!is_no_output_cmd("curl"));
        assert!(!is_no_output_cmd("cd"));
        assert!(!is_no_output_cmd("echo"));
        assert!(!is_no_output_cmd("export"));
    }

    #[test]
    fn strip_temporal_noise_removes_iso_timestamp() {
        let input = "Started at 2026-04-08T21:16:30Z, status: running";
        let out = strip_temporal_noise(input);
        assert!(!out.contains("2026-04-08"), "got: {}", out);
        assert!(out.contains("status: running"), "got: {}", out);
    }

    #[test]
    fn strip_temporal_noise_removes_hms_time() {
        let input = "Log entry 21:16:30 — process started";
        let out = strip_temporal_noise(input);
        assert!(!out.contains("21:16:30"), "got: {}", out);
        assert!(out.contains("process started"), "got: {}", out);
    }

    #[test]
    fn strip_temporal_noise_removes_uuid() {
        let input = "id=550e8400-e29b-41d4-a716-446655440000 status=ok";
        let out = strip_temporal_noise(input);
        assert!(!out.contains("550e8400"), "got: {}", out);
        assert!(out.contains("status=ok"), "got: {}", out);
    }

    #[test]
    fn strip_temporal_noise_preserves_semantic_content() {
        let input = "Build failed: missing symbol `foo` in auth.rs";
        let out = strip_temporal_noise(input);
        assert!(out.contains("Build failed"), "got: {}", out);
        assert!(out.contains("missing symbol"), "got: {}", out);
        assert!(out.contains("auth.rs"), "got: {}", out);
    }
}
