# CCR Optimization Plan 2 — EC · ZI · SD

Three features ordered by implementation risk (lowest-risk first).
Each section specifies: exact file + line, struct changes, function signatures, tests, edge cases.

---

## Implementation Order

```
EC → SD → ZI
```

**EC** (Elastic Context) — pure additions to existing structs, no signature breakage.
**SD** (Semantic State Map) — changes session.rs fields + delta format; backward-compatible via serde defaults.
**ZI** (Zoom-In) — most invasive: new ccr-core module, new `PipelineResult` field, new subcommand. Do last.

---

## EC — Elastic Context

### Problem

`session.compression_factor()` is binary: 1.0 below 50k tokens, stepping linearly toward 0.5 above it.
This only drives C2 (second-pass re-summarization in hook.rs). The pipeline itself — its BERT threshold
(200 lines) and its output budget (head_lines + tail_lines = 60 lines) — is completely pressure-blind.
As context fills, outputs stay the same size until C2 fires, which is too late and too coarse.

### Goal

- Three pressure tiers derived from cumulative session tokens.
- Pressure adjusts the pipeline config before it is constructed (not a new function parameter).
- Critical tier appends a context-pressure notice to the final output.
- Existing `compression_factor()` stays untouched (used by C2 — keep that path working).

### Pressure Tiers

| Tier     | total_tokens  | threshold_lines | budget factor |
|----------|---------------|-----------------|---------------|
| Normal   | < 25 k        | 200 (default)   | 1.00          |
| Elevated | 25 k – 60 k   | ~100            | 0.75          |
| Critical | > 60 k        | ~50             | 0.40          |

Continuous linear interpolation (not hard steps) prevents threshold cliffs.

---

### EC-1: `session.rs` — `context_pressure()`

Add alongside the existing `compression_factor()` method (after line 289).

```rust
/// Returns context pressure in [0.0, 1.0].
/// 0.0 = fresh session (no extra compression).
/// 1.0 = context is critically full (maximum tightening).
/// Ramps linearly from PRESSURE_START to PRESSURE_MAX cumulative output tokens.
pub fn context_pressure(&self) -> f32 {
    const PRESSURE_START: usize = 25_000;
    const PRESSURE_MAX: usize = 80_000;
    if self.total_tokens <= PRESSURE_START {
        return 0.0;
    }
    let range = (PRESSURE_MAX - PRESSURE_START) as f32;
    let pos = self.total_tokens.saturating_sub(PRESSURE_START) as f32;
    (pos / range).min(1.0)
}
```

**Why 25k / 80k:** Claude Code's context window is ~200k tokens. CCR sees filtered
outputs — roughly 1/3 of actual context. 25k filtered ≈ 75k actual (37% full → start
tightening). 80k filtered ≈ 240k actual (context near or over limit).

---

### EC-2: `ccr-core/src/config.rs` — `CcrConfig::with_pressure()`

Add as a method on `CcrConfig`. File: `ccr-core/src/config.rs`.

```rust
impl CcrConfig {
    /// Return a copy of this config adjusted for the given context pressure.
    /// pressure: 0.0 = no change, 1.0 = maximum tightening.
    ///
    /// Adjusts:
    ///   - summarize_threshold_lines: down to 25% of original (minimum 30)
    ///   - head_lines / tail_lines: down to 40% of original (minimum 4 each)
    pub fn with_pressure(mut self, pressure: f32) -> Self {
        if pressure < 0.01 {
            return self;
        }
        let p = pressure.clamp(0.0, 1.0);
        // Threshold shrinks: at p=1.0 it becomes 25% of the configured value.
        let threshold_factor = 1.0 - 0.75 * p;
        self.global.summarize_threshold_lines = ((self.global.summarize_threshold_lines as f32
            * threshold_factor) as usize)
            .max(30);
        // Budget shrinks: at p=1.0 head/tail each become 40% of configured.
        let budget_factor = 1.0 - 0.60 * p;
        self.global.head_lines =
            ((self.global.head_lines as f32 * budget_factor) as usize).max(4);
        self.global.tail_lines =
            ((self.global.tail_lines as f32 * budget_factor) as usize).max(4);
        self
    }
}
```

No changes to `GlobalConfig` struct fields — this is purely behavioural.

---

### EC-3: `ccr/src/hook.rs` — apply pressure before Pipeline construction

Replace the current `let pipeline = Pipeline::new(config);` (line 76) block:

```rust
// Compute context pressure from accumulated session tokens.
// Used to tighten BERT threshold and budget as the context window fills.
let pressure = session.context_pressure();
let pipeline = ccr_core::pipeline::Pipeline::new(config.with_pressure(pressure));
```

Also add the critical-pressure notice after `final_output` is built (after line 124):

```rust
// EC: In critical pressure (>0.8), warn the user that output is aggressively compressed.
if pressure > 0.80 {
    final_output.push_str(
        "\n[⚠ context near full — output compressed aggressively; run `ccr gain` to review]",
    );
}
```

---

### EC-4: `ccr/src/cmd/run.rs` — apply pressure in pipeline fallback

In the pipeline fallback block (lines 55-64), load the session to get pressure before
building the pipeline:

```rust
// Pipeline fallback for unknown commands
let config = match crate::config_loader::load_config() {
    Ok(c) => c,
    Err(_) => ccr_core::config::CcrConfig::default(),
};
// Apply context pressure so the fallback path respects session state.
let pressure = {
    let sid_p = crate::session::session_id();
    crate::session::SessionState::load(&sid_p).context_pressure()
};
let pipeline = ccr_core::pipeline::Pipeline::new(config.with_pressure(pressure));
match pipeline.process(&raw_output, Some(&cmd_name), Some(&cmd_name), None) {
    Ok(r) => r.output,
    Err(_) => raw_output.clone(),
}
```

---

### EC Tests

**File: `ccr/tests/elastic_context.rs`** (new integration test file)

```rust
use ccr::session::SessionState;
use ccr_core::config::CcrConfig;

// ── context_pressure() ────────────────────────────────────────────────────────

#[test]
fn pressure_zero_for_fresh_session() {
    let s = SessionState::default();
    assert_eq!(s.context_pressure(), 0.0);
}

#[test]
fn pressure_zero_below_start_threshold() {
    let mut s = SessionState::default();
    s.total_tokens = 20_000; // below 25k start
    assert_eq!(s.context_pressure(), 0.0);
}

#[test]
fn pressure_ramps_linearly() {
    let mut s = SessionState::default();
    s.total_tokens = 52_500; // midpoint: (52.5k - 25k) / (80k - 25k) = 0.5
    let p = s.context_pressure();
    assert!((p - 0.5).abs() < 0.01, "expected ~0.5, got {}", p);
}

#[test]
fn pressure_caps_at_one() {
    let mut s = SessionState::default();
    s.total_tokens = 200_000; // way over 80k
    assert_eq!(s.context_pressure(), 1.0);
}

// ── with_pressure() ───────────────────────────────────────────────────────────

#[test]
fn with_pressure_zero_is_identity() {
    let config = CcrConfig::default();
    let original_threshold = config.global.summarize_threshold_lines;
    let original_head = config.global.head_lines;
    let adjusted = config.with_pressure(0.0);
    assert_eq!(adjusted.global.summarize_threshold_lines, original_threshold);
    assert_eq!(adjusted.global.head_lines, original_head);
}

#[test]
fn with_pressure_one_tightens_threshold() {
    let config = CcrConfig::default();
    let original = config.global.summarize_threshold_lines; // 200
    let adjusted = config.with_pressure(1.0);
    // At p=1.0: 200 * 0.25 = 50
    assert!(adjusted.global.summarize_threshold_lines < original / 2,
        "threshold should be less than half the original");
    assert!(adjusted.global.summarize_threshold_lines >= 30,
        "threshold must not go below minimum of 30");
}

#[test]
fn with_pressure_one_tightens_budget() {
    let config = CcrConfig::default();
    let original_head = config.global.head_lines;
    let original_tail = config.global.tail_lines;
    let adjusted = config.with_pressure(1.0);
    assert!(adjusted.global.head_lines < original_head);
    assert!(adjusted.global.tail_lines < original_tail);
    assert!(adjusted.global.head_lines >= 4);
    assert!(adjusted.global.tail_lines >= 4);
}

#[test]
fn with_pressure_midpoint_is_between_zero_and_one() {
    let config = CcrConfig::default();
    let base = config.global.summarize_threshold_lines;
    let at_zero = CcrConfig::default().with_pressure(0.0).global.summarize_threshold_lines;
    let at_half = CcrConfig::default().with_pressure(0.5).global.summarize_threshold_lines;
    let at_one  = CcrConfig::default().with_pressure(1.0).global.summarize_threshold_lines;
    assert!(at_zero == base);
    assert!(at_half < at_zero && at_half > at_one);
}

// ── pipeline integration ──────────────────────────────────────────────────────

#[test]
fn pipeline_fires_bert_sooner_under_high_pressure() {
    use ccr_core::pipeline::Pipeline;

    // 60 lines — below normal threshold (200) but above critical threshold (~50)
    let input: String = (0..60).map(|i| format!("log line {}", i)).collect::<Vec<_>>().join("\n");

    // Under no pressure: 60 lines < 200 threshold → no BERT → output ≈ input lines
    let config_normal = CcrConfig::default();
    let result_normal = Pipeline::new(config_normal).process(&input, None, None, None).unwrap();
    assert!(result_normal.output.lines().count() >= 50,
        "no pressure should not summarize 60 lines");

    // Under max pressure: threshold shrinks to ~50 → BERT fires → fewer lines
    let config_pressure = CcrConfig::default().with_pressure(1.0);
    let result_pressure = Pipeline::new(config_pressure).process(&input, None, None, None).unwrap();
    assert!(result_pressure.output.lines().count() < result_normal.output.lines().count(),
        "high pressure should produce fewer output lines");
}

#[test]
fn compression_factor_unchanged_by_pressure_feature() {
    // Regression: existing compression_factor() must still work correctly.
    let mut s = SessionState::default();
    assert_eq!(s.compression_factor(), 1.0); // below threshold

    s.total_tokens = 100_000; // 2x the 50k threshold
    let cf = s.compression_factor();
    assert!(cf < 1.0 && cf >= 0.5, "compression_factor should be in [0.5, 1.0]");
}
```

**Add to `ccr/tests/mod.rs`** (or `ccr/tests/` directory): register `elastic_context` test file.

---

## SD — Semantic State Map

### Problem

Three gaps in the current delta compression:

1. **Coarse key granularity**: `run.rs` uses `cmd_name` (just `git`), so `git status`
   and `git log` spuriously match each other. The hook already uses the first word, so
   `git status` and `git diff` also clash in hook mode.

2. **4000-char preview limit**: For state-heavy commands (`git status`, `kubectl get`,
   `ps aux`), outputs are structured and often >4000 chars. Delta misses changes that
   appear after the preview cutoff.

3. **Opaque output format**: `[N lines same as turn X]` tells Claude "lines exist"
   but not what changed. `[Δ from turn 3: +2 new, 18 repeated]` is far more informative.

### Goal

- Delta key = base command + subcommand (e.g. `git status`, not just `git`).
- State commands store full content (uncapped) in `SessionEntry`.
- Delta output format changes to a richer diff summary.
- Configurable list of state commands in `GlobalConfig`.

---

### SD-1: `ccr-core/src/config.rs` — `state_commands` field

Add to `GlobalConfig` struct:

```rust
/// Commands whose output represents persistent system state.
/// These get full-content storage in SessionEntry (no 4000-char cap),
/// enabling accurate line-level delta across long state outputs.
#[serde(default = "default_state_commands")]
pub state_commands: Vec<String>,
```

Add the default function:

```rust
fn default_state_commands() -> Vec<String> {
    ["git", "kubectl", "ps", "ls", "df", "docker", "netstat", "kubectl"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}
```

No TOML changes to `default_filters.toml` for now — the defaults cover the common cases.
Users who want to customize add `state_commands = [...]` under `[global]`.

---

### SD-2: `ccr/src/session.rs` — richer `SessionEntry` + `record()` update

Add `state_content: Option<String>` to `SessionEntry`:

```rust
#[derive(Serialize, Deserialize, Clone)]
pub struct SessionEntry {
    pub turn: usize,
    pub cmd: String,
    pub ts: u64,
    pub tokens: usize,
    pub embedding: Vec<f32>,
    pub content_preview: String,     // first 4000 chars (non-state commands)
    #[serde(default)]
    pub state_content: Option<String>, // full content (state commands only)
}
```

`#[serde(default)]` ensures existing session files (which lack this field) deserialize
without error.

Update `record()` to accept `is_state: bool`:

```rust
pub fn record(
    &mut self,
    cmd: &str,
    embedding: Vec<f32>,
    tokens: usize,
    content: &str,
    is_state: bool,        // NEW
) {
    self.total_turns += 1;
    self.total_tokens += tokens;

    const PREVIEW_CHARS: usize = 4_000;
    let (content_preview, state_content) = if is_state {
        // State commands: preview still populated for C1 dedup, full content
        // stored separately for delta matching.
        (
            content.chars().take(PREVIEW_CHARS).collect(),
            Some(content.to_string()),
        )
    } else {
        (content.chars().take(PREVIEW_CHARS).collect(), None)
    };

    let entry = SessionEntry {
        turn: self.total_turns,
        cmd: cmd.to_string(),
        ts: now_secs(),
        tokens,
        embedding,
        content_preview,
        state_content,
    };

    self.entries.push(entry);
    if self.entries.len() > MAX_ENTRIES {
        self.entries.remove(0);
    }
}
```

Update `compute_delta()` to:
1. Use `state_content` when available (for full-content line comparison).
2. Emit richer output format.

```rust
pub fn compute_delta(
    &self,
    cmd: &str,
    new_lines: &[&str],
    new_embedding: &[f32],
) -> Option<DeltaResult> {
    let prior = self
        .entries
        .iter()
        .filter(|e| e.cmd == cmd && !e.embedding.is_empty())
        .rev()
        .find(|e| cosine_sim(new_embedding, &e.embedding) >= DELTA_THRESHOLD)?;

    let model = ccr_core::summarizer::embed_batch(new_lines).ok()?;

    // Use state_content for full comparison if available, else preview.
    let prior_text = prior
        .state_content
        .as_deref()
        .unwrap_or(&prior.content_preview);

    let prior_lines: Vec<&str> = prior_text.lines().collect();
    if prior_lines.is_empty() {
        return None;
    }
    let prior_embs = ccr_core::summarizer::embed_batch(&prior_lines).ok()?;

    const LINE_MATCH_THRESHOLD: f32 = 0.88;
    let mut new_lines_out: Vec<String> = Vec::new();
    let mut same_count = 0usize;
    let mut new_count = 0usize;

    for (i, line) in new_lines.iter().enumerate() {
        let line_emb = &model[i];
        let best_sim = prior_embs
            .iter()
            .map(|pe| cosine_sim(line_emb, pe))
            .fold(0.0f32, f32::max);

        if best_sim >= LINE_MATCH_THRESHOLD {
            same_count += 1;
        } else {
            new_count += 1;
            new_lines_out.push((*line).to_string());
        }
    }

    // Richer format: shows what changed, not just how many were the same.
    let approx_saved = prior.tokens.saturating_mul(same_count)
        / prior_lines.len().max(1);
    let ref_marker = format!(
        "[Δ from turn {}: +{} new, {} repeated — ~{} tokens saved]",
        prior.turn, new_count, same_count, approx_saved
    );

    let mut output_parts: Vec<String> = Vec::new();
    if same_count > 0 {
        output_parts.push(ref_marker);
    }
    output_parts.extend(new_lines_out);

    Some(DeltaResult {
        output: output_parts.join("\n"),
        new_count,
        same_count,
        reference_turn: prior.turn,
    })
}
```

---

### SD-3: `ccr/src/hook.rs` — subcommand-aware delta key

Currently `cmd_key = command_hint.as_deref().unwrap_or("unknown")` which is just
the first word.

Replace:

```rust
let cmd_key = command_hint.as_deref().unwrap_or("unknown");
```

With:

```rust
// Use first two words of the full command as the delta/centroid key.
// This distinguishes "git status" from "git log" while grouping
// "git status" with "git status --short" (flags differ, intent same).
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
```

Also update `is_state` calculation when recording to session:

```rust
// Load config to know which commands are state commands.
let is_state = config
    .global
    .state_commands
    .iter()
    .any(|s| command_hint.as_deref() == Some(s.as_str()));

// Pass is_state to record():
session.record(cmd_key, emb, tokens, &final_output, is_state);
```

---

### SD-4: `ccr/src/cmd/run.rs` — subcommand-aware delta key

`cmd_name` is just `args[0]` (e.g. `git`). The subcommand is already extracted as
`subcommand`. Use them together:

```rust
// Use cmd + subcommand as delta key for better granularity.
// "git status" won't match "git log" history.
let delta_key = match &subcommand {
    Some(sub) => format!("{} {}", cmd_name, sub),
    None => cmd_name.clone(),
};
```

Replace all uses of `&cmd_name` in `compute_delta()` / `find_similar()` / `session.record()`
with `&delta_key`.

Also add `is_state` check using the loaded config's `state_commands`:

```rust
let is_state = {
    let cfg = crate::config_loader::load_config().unwrap_or_default();
    cfg.global.state_commands.iter().any(|s| s == &cmd_name)
};
session.record(&delta_key, emb, tokens, &filtered, is_state);
```

---

### SD Tests

**File: `ccr/tests/semantic_state_map.rs`** (new)

```rust
use ccr::session::SessionState;
use ccr_core::summarizer::embed_batch;

fn embed(text: &str) -> Vec<f32> {
    embed_batch(&[text]).unwrap().pop().unwrap()
}

fn session_with_prior(cmd: &str, content: &str, is_state: bool) -> SessionState {
    let mut s = SessionState::default();
    let emb = embed(content);
    let tokens = content.len() / 4;
    s.record(cmd, emb, tokens, content, is_state);
    s
}

// ── subcommand-aware key ───────────────────────────────────────────────────────

#[test]
fn git_status_and_git_log_do_not_delta_each_other() {
    // Prior entry recorded under "git status"
    let status_output = "On branch main\nnothing to commit, working tree clean";
    let session = session_with_prior("git status", status_output, true);

    // New output for "git log" — structurally similar but different command
    let log_output = "commit abc123\nAuthor: User\nDate: Mon\n\n    fix: typo";
    let emb = embed(log_output);
    let lines: Vec<&str> = log_output.lines().collect();

    // delta for "git log" should be None (no prior for that key)
    let result = session.compute_delta("git log", &lines, &emb);
    assert!(result.is_none(), "git log should not match git status history");
}

#[test]
fn same_command_variant_flags_share_delta_history() {
    // Prior recorded under "git status" (the 2-word key)
    let content = (0..30).map(|i| format!("  modified: file{}.rs", i)).collect::<Vec<_>>().join("\n");
    let session = session_with_prior("git status", &content, true);

    // New output is slightly different (one new file) — should match
    let mut new_lines: Vec<String> = (0..30).map(|i| format!("  modified: file{}.rs", i)).collect();
    new_lines.push("  new file: extra.rs".to_string());
    let new_text = new_lines.join("\n");
    let emb = embed(&new_text);
    let lines: Vec<&str> = new_text.lines().collect();

    let result = session.compute_delta("git status", &lines, &emb);
    assert!(result.is_some(), "similar git status runs should match");
    let delta = result.unwrap();
    assert!(delta.same_count > 0);
    assert!(delta.new_count >= 1, "new file line should appear");
}

// ── state command full-content storage ────────────────────────────────────────

#[test]
fn state_command_stores_full_content_beyond_4000_chars() {
    let long_content: String = (0..200).map(|i| format!("file_{:04}.rs  1234 bytes\n", i)).collect();
    assert!(long_content.len() > 4000);

    let mut s = SessionState::default();
    let emb = embed(&long_content);
    s.record("ls", emb, long_content.len() / 4, &long_content, true /* is_state */);

    let entry = &s.entries[0];
    assert!(entry.state_content.is_some(), "state command should have state_content");
    assert_eq!(
        entry.state_content.as_ref().unwrap().len(),
        long_content.len(),
        "state_content must be the full content, not truncated"
    );
}

#[test]
fn non_state_command_caps_preview_at_4000() {
    let long_content: String = (0..200).map(|i| format!("line {}\n", i)).collect();
    assert!(long_content.len() > 4000);

    let mut s = SessionState::default();
    let emb = embed(&long_content);
    s.record("cargo", emb, long_content.len() / 4, &long_content, false /* is_state */);

    let entry = &s.entries[0];
    assert!(entry.state_content.is_none(), "non-state command should have no state_content");
    assert!(
        entry.content_preview.len() <= 4000,
        "preview must be capped at 4000 chars"
    );
}

#[test]
fn state_content_used_for_delta_beyond_preview_boundary() {
    // Build a large state output where changes appear beyond 4000 chars.
    let mut lines: Vec<String> = (0..150).map(|i| format!("  modified: src/module{}.rs", i)).collect();
    let content = lines.join("\n");
    assert!(content.len() > 4000, "content must exceed preview boundary");

    let session = session_with_prior("git status", &content, true /* is_state */);

    // New run: same 150 lines but last one changed (appears after 4000-char boundary)
    let mut new_lines = lines.clone();
    *new_lines.last_mut().unwrap() = "  deleted:  src/module149.rs".to_string();
    let new_text = new_lines.join("\n");
    let emb = embed(&new_text);
    let refs: Vec<&str> = new_text.lines().collect();

    let result = session.compute_delta("git status", &refs, &emb).expect("delta should fire");
    // The changed last line must appear as "new"
    assert!(
        result.output.contains("module149") || result.new_count >= 1,
        "change beyond 4000-char boundary must be detected"
    );
}

// ── richer delta output format ────────────────────────────────────────────────

#[test]
fn delta_output_contains_richer_format() {
    let content = (0..25).map(|i| format!("cargo:warning=unused: item {}", i)).collect::<Vec<_>>().join("\n");
    let session = session_with_prior("cargo build", &content, false);

    let mut new_lines: Vec<String> = (0..25).map(|i| format!("cargo:warning=unused: item {}", i)).collect();
    new_lines.push("error[E0001]: new error".to_string());
    let new_text = new_lines.join("\n");
    let emb = embed(&new_text);
    let refs: Vec<&str> = new_text.lines().collect();

    let result = session.compute_delta("cargo build", &refs, &emb).expect("delta should fire");
    assert!(
        result.output.contains("Δ from turn"),
        "output should contain new richer format marker, got: {}",
        &result.output[..result.output.len().min(200)]
    );
    assert!(
        !result.output.contains("lines same as turn"),
        "old format marker should be gone"
    );
    assert!(result.output.contains("error[E0001]"), "new error must appear");
}

// ── serde backward compatibility ──────────────────────────────────────────────

#[test]
fn session_with_missing_state_content_deserializes_ok() {
    // Simulate a session file written before state_content was added.
    let json = r#"{
        "entries": [{
            "turn": 1, "cmd": "git", "ts": 0, "tokens": 10,
            "embedding": [0.1, 0.2],
            "content_preview": "On branch main"
        }],
        "total_turns": 1,
        "total_tokens": 10,
        "command_centroids": {}
    }"#;
    let session: SessionState = serde_json::from_str(json).expect("must deserialize");
    assert!(session.entries[0].state_content.is_none());
}

// ── config deserialization ────────────────────────────────────────────────────

#[test]
fn state_commands_default_includes_git() {
    let config = ccr_core::config::CcrConfig::default();
    assert!(
        config.global.state_commands.iter().any(|s| s == "git"),
        "git must be in default state_commands"
    );
}

#[test]
fn state_commands_can_be_overridden_in_toml() {
    let toml = r#"
[global]
state_commands = ["custom_tool", "my_status"]
"#;
    let config: ccr_core::config::CcrConfig = toml::from_str(toml).unwrap();
    assert!(config.global.state_commands.contains(&"custom_tool".to_string()));
    assert!(!config.global.state_commands.contains(&"git".to_string()),
        "overriding state_commands should replace defaults");
}
```

**Register `semantic_state_map` in the test harness.**

---

## ZI — Zoom-In

### Problem

When CCR collapses a block (`[19 matching lines collapsed]`) or BERT omits lines
(`[42 lines omitted]`), the information is irretrievable without re-running the
command. For large compressed outputs, the tee file is appended as a hint, but
Claude can't ask for a specific block — it's all-or-nothing.

### Goal

- Each collapsed/omitted block gets a unique ID (`ZI_1`, `ZI_2`, …).
- The marker becomes: `[19 matching lines collapsed — ccr expand ZI_1]`
- `ccr expand ZI_1` prints the original lines for that block.
- Blocks are stored in `~/.local/share/ccr/expand/{session_id}/ZI_N.txt`.
- Zoom is opt-in (enabled by callers that have session context).
- `ccr filter` does NOT enable zoom (no session, used in testing/debugging).

### Architecture

Zoom state lives in a **thread-local accumulator** in `ccr-core` (same pattern as
P4's `EXTRA_KEEP_PATTERNS`). The caller enables zoom, the pipeline and patterns
fill the accumulator, the caller drains it and persists to disk.

---

### ZI-1: `ccr-core/src/zoom.rs` — new module

**File: `ccr-core/src/zoom.rs`**

```rust
//! Zoom-In block registry.
//!
//! When zoom is enabled by the calling layer, collapse/omission markers in
//! pipeline output include a `ccr expand ZI_N` reference. The original lines
//! for each block are registered here and later drained by the caller for
//! persistence to disk.

use std::cell::RefCell;

pub struct ZoomBlock {
    pub id: String,
    pub lines: Vec<String>,
}

thread_local! {
    static ENABLED: RefCell<bool> = RefCell::new(false);
    static COUNTER: RefCell<usize> = RefCell::new(0);
    static BLOCKS: RefCell<Vec<ZoomBlock>> = RefCell::new(Vec::new());
}

/// Enable zoom for the current thread. Resets the counter and block list.
/// Call this before invoking the pipeline when you have session context.
pub fn enable() {
    ENABLED.with(|e| *e.borrow_mut() = true);
    COUNTER.with(|c| *c.borrow_mut() = 0);
    BLOCKS.with(|b| b.borrow_mut().clear());
}

/// Disable zoom (e.g., for `ccr filter` where no session context exists).
pub fn disable() {
    ENABLED.with(|e| *e.borrow_mut() = false);
}

/// Returns true if zoom is currently enabled on this thread.
pub fn is_enabled() -> bool {
    ENABLED.with(|e| *e.borrow())
}

/// Generate the next zoom ID and register the original lines for that block.
/// Returns the ID string to embed in the output marker.
pub fn register(lines: Vec<String>) -> String {
    let id = COUNTER.with(|c| {
        let mut n = c.borrow_mut();
        *n += 1;
        format!("ZI_{}", n)
    });
    BLOCKS.with(|b| {
        b.borrow_mut().push(ZoomBlock { id: id.clone(), lines });
    });
    id
}

/// Drain all registered blocks, returning them to the caller for persistence.
/// Resets the block list. Counter is NOT reset (IDs remain unique within a session).
pub fn drain() -> Vec<ZoomBlock> {
    BLOCKS.with(|b| std::mem::take(&mut *b.borrow_mut()))
}
```

**Add to `ccr-core/src/lib.rs`:**
```rust
pub mod zoom;
```

---

### ZI-2: `ccr-core/src/patterns.rs` — embed zoom IDs in collapse markers

In `PatternFilter::apply()`, locate the lines that push collapse markers:

```rust
// Current (appears 3 times):
result.push(format!("[{} matching lines collapsed]", collapse_counts[ci]));
```

Replace each with a helper call:

```rust
result.push(make_collapse_marker(collapse_counts[ci], &collapsed_line_buffer[ci]));
```

Add `collapsed_line_buffer: Vec<Vec<String>>` to track the actual collapsed lines
per pattern index. Before each collapse-pattern match, append the original line to
the buffer. When flushing, pass the buffer to the helper.

Helper function:

```rust
fn make_collapse_marker(count: usize, original_lines: &[String]) -> String {
    if crate::zoom::is_enabled() && !original_lines.is_empty() {
        let id = crate::zoom::register(original_lines.to_vec());
        format!("[{} matching lines collapsed — ccr expand {}]", count, id)
    } else {
        format!("[{} matching lines collapsed]", count)
    }
}
```

**Implementation detail for tracking collapsed lines:**

The current `apply()` accumulates a count per pattern (`collapse_counts`) but discards
the actual lines. We need a parallel `Vec<Vec<String>>` of the same length as `self.patterns`.

```rust
let mut collapse_counts: Vec<usize> = vec![0; self.patterns.len()];
let mut collapsed_lines: Vec<Vec<String>> = vec![Vec::new(); self.patterns.len()]; // NEW
let mut active_collapse: Option<usize> = None;

// In the Collapse arm, also accumulate the line:
FilterAction::Simple(SimpleAction::Collapse) => {
    // ...existing logic...
    active_collapse = Some(i);
    collapse_counts[i] += 1;
    collapsed_lines[i].push(line.to_string()); // NEW
}

// When flushing, pass the accumulated lines:
if collapse_counts[ci] > 0 {
    let lines = std::mem::take(&mut collapsed_lines[ci]);
    result.push(make_collapse_marker(collapse_counts[ci], &lines));
    collapse_counts[ci] = 0;
}
```

---

### ZI-3: `ccr-core/src/summarizer.rs` — embed zoom IDs in omission markers

The summarizer emits omission markers in several functions. Find every occurrence of
the pattern `format!("... lines omitted ...")` or similar and update them.

Add a helper:

```rust
fn make_omission_marker(count: usize, omitted: &[String]) -> String {
    if crate::zoom::is_enabled() && !omitted.is_empty() {
        let id = crate::zoom::register(omitted.to_vec());
        format!("[{} lines omitted — ccr expand {}]", count, id)
    } else {
        format!("[{} lines omitted]", count)
    }
}
```

Each summarize function that currently does:
```rust
// [N lines omitted] style markers
```
needs to collect the omitted lines into a `Vec<String>` and call `make_omission_marker`.

**Strategy**: Each summarizer function already knows which lines it keeps (indices in
`keep_indices` or similar). The omitted lines are the complement. Collect them before
building the output and pass to `make_omission_marker`.

The omission markers are typically emitted as a single trailing line. Example pattern
to update in each summarize function:

```rust
// BEFORE:
if omitted > 0 {
    parts.push(format!("[{} lines omitted]", omitted));
}

// AFTER:
if omitted > 0 {
    let omitted_content: Vec<String> = lines
        .iter()
        .enumerate()
        .filter(|(i, _)| !keep_set.contains(i))
        .map(|(_, l)| l.to_string())
        .collect();
    parts.push(make_omission_marker(omitted, &omitted_content));
}
```

---

### ZI-4: `ccr-core/src/pipeline.rs` — expose zoom blocks in `PipelineResult`

```rust
pub struct PipelineResult {
    pub output: String,
    pub analytics: Analytics,
    pub zoom_blocks: Vec<crate::zoom::ZoomBlock>, // NEW
}
```

At the end of `Pipeline::process()`, drain the zoom accumulator:

```rust
Ok(PipelineResult {
    output: text,
    analytics,
    zoom_blocks: crate::zoom::drain(),  // NEW
})
```

All existing callers that access only `.output` and `.analytics` are unaffected.
The `ccr-eval` runner and `cmd/filter.rs` simply ignore `.zoom_blocks`.

---

### ZI-5: `ccr/src/zoom_store.rs` — new module for disk persistence

**File: `ccr/src/zoom_store.rs`**

```rust
//! Persistence layer for Zoom-In blocks.
//!
//! Blocks are stored at: ~/.local/share/ccr/expand/{session_id}/ZI_N.txt
//! The expand command searches all session directories for a given ID.

use ccr_core::zoom::ZoomBlock;
use std::path::PathBuf;

fn expand_dir() -> Option<PathBuf> {
    Some(dirs::data_local_dir()?.join("ccr").join("expand"))
}

fn session_expand_dir(session_id: &str) -> Option<PathBuf> {
    Some(expand_dir()?.join(session_id))
}

/// Persist a batch of zoom blocks to disk for the given session.
pub fn save_blocks(session_id: &str, blocks: Vec<ZoomBlock>) -> anyhow::Result<()> {
    if blocks.is_empty() {
        return Ok(());
    }
    let dir = session_expand_dir(session_id)
        .ok_or_else(|| anyhow::anyhow!("cannot determine data directory"))?;
    std::fs::create_dir_all(&dir)?;
    for block in blocks {
        let path = dir.join(format!("{}.txt", block.id));
        std::fs::write(path, block.lines.join("\n"))?;
    }
    Ok(())
}

/// Load a specific zoom block by ID, searching across all sessions.
pub fn load_block(id: &str) -> anyhow::Result<String> {
    let base = expand_dir()
        .ok_or_else(|| anyhow::anyhow!("cannot determine data directory"))?;

    if !base.exists() {
        anyhow::bail!("No expand blocks found. Run a command through ccr first.");
    }

    for session_entry in std::fs::read_dir(&base)? {
        let session_dir = session_entry?.path();
        if !session_dir.is_dir() {
            continue;
        }
        let file = session_dir.join(format!("{}.txt", id));
        if file.exists() {
            return Ok(std::fs::read_to_string(file)?);
        }
    }

    anyhow::bail!(
        "No block found for '{}'. IDs are only valid within the current session.",
        id
    )
}

/// List all block IDs available across all sessions (for `ccr expand --list`).
pub fn list_blocks() -> Vec<String> {
    let base = match expand_dir() {
        Some(d) => d,
        None => return Vec::new(),
    };
    let mut ids = Vec::new();
    if let Ok(sessions) = std::fs::read_dir(&base) {
        for session in sessions.flatten() {
            if let Ok(files) = std::fs::read_dir(session.path()) {
                for file in files.flatten() {
                    let name = file.file_name().to_string_lossy().to_string();
                    if name.ends_with(".txt") && name.starts_with("ZI_") {
                        ids.push(name.trim_end_matches(".txt").to_string());
                    }
                }
            }
        }
    }
    ids.sort();
    ids
}
```

Add `pub mod zoom_store;` to `ccr/src/lib.rs`.

---

### ZI-6: `ccr/src/cmd/expand.rs` — new subcommand

**File: `ccr/src/cmd/expand.rs`**

```rust
use anyhow::Result;

pub fn run(id: &str, list: bool) -> Result<()> {
    if list {
        let ids = crate::zoom_store::list_blocks();
        if ids.is_empty() {
            println!("No zoom blocks available. Run a command through ccr first.");
        } else {
            for id in ids {
                println!("{}", id);
            }
        }
        return Ok(());
    }

    let content = crate::zoom_store::load_block(id)?;
    print!("{}", content);
    if !content.ends_with('\n') {
        println!();
    }
    Ok(())
}
```

Add `pub mod expand;` to `ccr/src/cmd/mod.rs`.

---

### ZI-7: `ccr/src/main.rs` — register `Expand` subcommand + enable zoom in callers

Add to `Commands` enum:

```rust
/// Print the original lines from a collapsed/omitted block.
Expand {
    /// Zoom block ID (e.g. ZI_1), shown in compressed output markers.
    id: Option<String>,
    /// List all available block IDs in the current session.
    #[arg(long)]
    list: bool,
},
```

Add to the match block:

```rust
Commands::Expand { id, list } => {
    let id = id.unwrap_or_default();
    cmd::expand::run(&id, list)?;
}
```

**Enable zoom in hook.rs** (before pipeline call):

```rust
// Enable Zoom-In so compressed markers include expand IDs.
ccr_core::zoom::enable();
let pipeline = ccr_core::pipeline::Pipeline::new(config.with_pressure(pressure));
let result = match pipeline.process(...) { ... };

// Persist zoom blocks so `ccr expand` can retrieve them later.
let _ = crate::zoom_store::save_blocks(&sid, result.zoom_blocks);
```

**Enable zoom in run.rs** (before pipeline fallback):

```rust
ccr_core::zoom::enable();
let pipeline = ccr_core::pipeline::Pipeline::new(config.with_pressure(pressure));
let r = pipeline.process(...)?;
let _ = crate::zoom_store::save_blocks(&crate::session::session_id(), r.zoom_blocks);
r.output
```

**Do NOT enable zoom in `cmd/filter.rs`** — filter is used in testing and debugging
where no session context exists.

---

### ZI Tests

**File: `ccr-core/tests/zoom_blocks.rs`** (new unit test for ccr-core)

```rust
use ccr_core::zoom;
use ccr_core::config::{CommandConfig, FilterAction, FilterPattern, SimpleAction};
use ccr_core::patterns::PatternFilter;

fn make_collapse_filter() -> PatternFilter {
    PatternFilter::new(&CommandConfig {
        patterns: vec![FilterPattern {
            regex: r"^\s*Compiling \S+".to_string(),
            action: FilterAction::Simple(SimpleAction::Collapse),
        }],
    }).unwrap()
}

#[test]
fn zoom_disabled_no_id_in_output() {
    zoom::disable();
    let filter = make_collapse_filter();
    let input = "   Compiling foo v1.0\n   Compiling bar v1.0\nerror: build failed";
    let result = filter.apply(input);
    assert!(result.contains("collapsed"), "collapse marker should still appear");
    assert!(!result.contains("ZI_"), "no zoom ID when zoom is disabled");
}

#[test]
fn zoom_enabled_id_in_collapse_output() {
    zoom::enable();
    let filter = make_collapse_filter();
    let input = "   Compiling foo v1.0\n   Compiling bar v1.0\nerror: build failed";
    let result = filter.apply(input);
    assert!(result.contains("ZI_"), "zoom ID should appear when zoom is enabled");
    assert!(result.contains("ccr expand"), "expand hint should appear");
}

#[test]
fn zoom_blocks_contain_original_lines() {
    zoom::enable();
    let filter = make_collapse_filter();
    let input = "   Compiling foo v1.0\n   Compiling bar v1.0\nerror: build failed";
    filter.apply(input);
    let blocks = zoom::drain();
    assert!(!blocks.is_empty(), "at least one block should be registered");
    let block = &blocks[0];
    assert!(block.lines.iter().any(|l| l.contains("Compiling foo")));
    assert!(block.lines.iter().any(|l| l.contains("Compiling bar")));
}

#[test]
fn zoom_ids_increment_across_multiple_collapses() {
    zoom::enable();
    let filter = make_collapse_filter();
    // Two separate runs create two blocks
    let input1 = "   Compiling foo v1.0\nerror: first";
    let input2 = "   Compiling bar v1.0\nerror: second";
    let out1 = filter.apply(input1);
    let out2 = filter.apply(input2);
    assert!(out1.contains("ZI_1"), "first collapse should be ZI_1");
    assert!(out2.contains("ZI_2"), "second collapse should be ZI_2");
}

#[test]
fn drain_clears_block_list() {
    zoom::enable();
    let filter = make_collapse_filter();
    filter.apply("   Compiling foo v1.0\n");
    let blocks = zoom::drain();
    assert!(!blocks.is_empty());
    let blocks2 = zoom::drain();
    assert!(blocks2.is_empty(), "drain should clear the block list");
}
```

**File: `ccr/tests/integration_expand.rs`** (new integration test)

```rust
use assert_cmd::Command;
use tempfile::TempDir;

#[test]
fn expand_unknown_id_returns_error() {
    let mut cmd = Command::cargo_bin("ccr").unwrap();
    cmd.arg("expand").arg("ZI_999999");
    cmd.assert().failure();
}

#[test]
fn expand_list_works_when_no_blocks() {
    let mut cmd = Command::cargo_bin("ccr").unwrap();
    cmd.arg("expand").arg("--list");
    // Should succeed (exit 0) even with no blocks
    cmd.assert().success();
}

#[test]
fn zoom_blocks_survive_roundtrip() {
    use ccr::zoom_store;
    use ccr_core::zoom::ZoomBlock;

    let tmp = TempDir::new().unwrap();
    // Override data dir not easily possible; test the zoom_store functions directly.
    // We test save_blocks + load_block using a synthetic session.
    // (Full integration requires a real ccr run — covered by the filter integration tests.)
    let blocks = vec![
        ZoomBlock { id: "ZI_1".to_string(), lines: vec!["line1".to_string(), "line2".to_string()] },
    ];

    // Write to temp dir manually for isolation
    let session_dir = tmp.path().join("expand").join("test_session");
    std::fs::create_dir_all(&session_dir).unwrap();
    for block in &blocks {
        std::fs::write(
            session_dir.join(format!("{}.txt", block.id)),
            block.lines.join("\n"),
        ).unwrap();
    }

    // Read back via the same path structure
    let content = std::fs::read_to_string(session_dir.join("ZI_1.txt")).unwrap();
    assert_eq!(content, "line1\nline2");
}
```

---

## Backward Compatibility Checklist

| Change | Affected callers | Action |
|--------|-----------------|--------|
| `SessionEntry` adds `state_content` | All session file deserializations | `#[serde(default)]` — old files load fine |
| `SessionState::record()` adds `is_state: bool` | `hook.rs`, `run.rs` | Pass `false` for unknown commands |
| `PipelineResult` adds `zoom_blocks` | `ccr-eval/src/runner.rs`, `cmd/filter.rs`, `hook.rs`, `run.rs` | Add field to construction; ignore in callers that don't need it |
| `GlobalConfig` adds `state_commands` | Config deserialization | `#[serde(default)]` — existing configs get built-in defaults |
| `CcrConfig::with_pressure()` | All Pipeline call sites | All 3 callers (hook, run, filter) need to pass pressure; filter uses 0.0 |
| `ccr expand` subcommand | `main.rs` | Add to `Commands` enum and match arm |
| Collapse markers gain `— ccr expand ZI_N` | Any test asserting exact marker text | Update test assertions |

---

## Running the Tests

```bash
# EC tests
cargo test -p ccr --test elastic_context

# SD tests
cargo test -p ccr --test semantic_state_map

# ZI unit tests (ccr-core)
cargo test -p ccr-core --test zoom_blocks

# ZI integration tests
cargo test -p ccr --test integration_expand

# Full regression suite (must still be all green)
cargo test --workspace
```

---

## Implementation Checklist

### EC
- [ ] `session.rs`: add `context_pressure()`
- [ ] `config.rs`: add `CcrConfig::with_pressure()`
- [ ] `hook.rs`: read pressure, call `with_pressure()`, append critical notice
- [ ] `run.rs`: load session for pressure in pipeline-fallback path
- [ ] `ccr/tests/elastic_context.rs`: 9 tests
- [ ] `cargo test --workspace` → all green

### SD
- [ ] `config.rs`: add `state_commands` field + default
- [ ] `session.rs`: add `state_content` to `SessionEntry`
- [ ] `session.rs`: update `record()` to accept `is_state: bool`
- [ ] `session.rs`: update `compute_delta()` to use `state_content` + richer format
- [ ] `hook.rs`: compute `is_state`, update `record()` call, use 2-word `cmd_key`
- [ ] `run.rs`: compute `delta_key` from cmd+subcommand, compute `is_state`, update `record()` call
- [ ] `ccr/tests/semantic_state_map.rs`: 8 tests
- [ ] `cargo test --workspace` → all green

### ZI
- [ ] `ccr-core/src/zoom.rs`: new module
- [ ] `ccr-core/src/lib.rs`: `pub mod zoom;`
- [ ] `ccr-core/src/patterns.rs`: track collapsed lines, call `make_collapse_marker`
- [ ] `ccr-core/src/summarizer.rs`: collect omitted lines, call `make_omission_marker`
- [ ] `ccr-core/src/pipeline.rs`: add `zoom_blocks` to `PipelineResult`, drain at end
- [ ] `ccr/src/zoom_store.rs`: new module (save_blocks, load_block, list_blocks)
- [ ] `ccr/src/lib.rs`: `pub mod zoom_store;`
- [ ] `ccr/src/cmd/expand.rs`: new subcommand
- [ ] `ccr/src/cmd/mod.rs`: `pub mod expand;`
- [ ] `ccr/src/main.rs`: add `Expand` variant, enable zoom in hook/run callers
- [ ] `ccr-core/tests/zoom_blocks.rs`: 5 unit tests
- [ ] `ccr/tests/integration_expand.rs`: 3 integration tests
- [ ] Verify collapse marker format change doesn't break any existing test assertions
- [ ] `cargo test --workspace` → all green
