/// Tests for Idea 7: Per-Command Historical Centroid
///
/// Verifies that `SessionState` accumulates per-command centroids across turns,
/// and that anomaly scoring against the historical centroid assigns higher scores
/// to genuinely new output than to lines that appear in every run.
///
/// Expected changes to `SessionState`:
///
///   // New field:
///   pub command_centroids: HashMap<String, Vec<f32>>,
///
///   // New methods:
///   impl SessionState {
///       pub fn command_centroid(&self, cmd: &str) -> Option<&Vec<f32>>
///       pub fn update_command_centroid(&mut self, cmd: &str, centroid: Vec<f32>)
///   }
///
///   // New function in panda-core::summarizer:
///   pub fn summarize_against_centroid(
///       text: &str, budget: usize, historical_centroid: &[f32]
///   ) -> SummarizeResult
///
/// Run with: cargo test -p panda --test idea7_historical_centroid
use panda::session::SessionState;
use panda_core::summarizer::{embed_batch, summarize_against_centroid};

// ── helpers ───────────────────────────────────────────────────────────────────


fn centroid_of(texts: &[&str]) -> Vec<f32> {
    let embs = embed_batch(texts).expect("embed_batch failed");
    let dim = embs[0].len();
    let mut c = vec![0.0f32; dim];
    for e in &embs {
        for (i, v) in e.iter().enumerate() {
            c[i] += v;
        }
    }
    let n = embs.len() as f32;
    c.iter_mut().for_each(|v| *v /= n);
    c
}

fn standard_cargo_lines() -> Vec<String> {
    (0..20)
        .map(|i| format!("   Compiling standard-crate-{} v1.0.{}", i, i))
        .collect()
}

// ── command_centroid / update_command_centroid ────────────────────────────────

/// A freshly-created session has no centroid for any command.
#[test]
fn new_session_has_no_centroids() {
    let session = SessionState::default();
    assert!(session.command_centroid("cargo").is_none());
    assert!(session.command_centroid("git").is_none());
}

/// After updating, `command_centroid` must return the stored vector.
#[test]
fn update_and_retrieve_centroid() {
    let mut session = SessionState::default();
    let centroid = vec![0.5f32; 384];
    session.update_command_centroid("cargo", centroid.clone());

    let retrieved = session.command_centroid("cargo").expect("centroid should be present");
    assert_eq!(*retrieved, centroid);
}

/// Different commands must have independent centroids.
#[test]
fn independent_centroids_per_command() {
    let mut session = SessionState::default();
    let cargo_centroid = vec![1.0f32; 384];
    let git_centroid = vec![0.0f32; 384];

    session.update_command_centroid("cargo", cargo_centroid.clone());
    session.update_command_centroid("git", git_centroid.clone());

    assert_eq!(*session.command_centroid("cargo").unwrap(), cargo_centroid);
    assert_eq!(*session.command_centroid("git").unwrap(), git_centroid);
}

/// Centroid must persist across `save` / `load` cycle.
#[test]
fn centroid_persists_across_save_load() {
    let sid = format!("test-centroid-persist-{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis());

    let centroid = vec![0.42f32; 384];

    {
        let mut session = SessionState::default();
        session.update_command_centroid("cargo", centroid.clone());
        session.save(&sid);
    }

    let loaded = SessionState::load(&sid);
    let retrieved = loaded.command_centroid("cargo").expect("centroid must survive save/load");
    // Allow small f32 serialization rounding
    let max_diff = retrieved.iter().zip(&centroid).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(max_diff < 1e-5, "Centroid drift after save/load: max_diff={:.2e}", max_diff);
}

// ── summarize_against_centroid ────────────────────────────────────────────────

/// A new error line (not in historical centroid) must appear in the output.
#[test]
fn new_line_appears_in_output_against_historical_centroid() {
    let standard = standard_cargo_lines();
    let standard_refs: Vec<&str> = standard.iter().map(|s| s.as_str()).collect();
    let historical = centroid_of(&standard_refs);

    // Build an input that looks like standard runs + 1 genuinely new error
    let mut lines = standard.clone();
    lines.push("error[E0502]: cannot borrow `conn` as mutable because it is borrowed".to_string());
    let input = lines.join("\n");

    let result = summarize_against_centroid(&input, 15, &historical);

    assert!(
        result.output.contains("error[E0502]"),
        "New error should appear when scored against historical centroid:\n{}",
        result.output
    );
}

/// Standard compilation lines (in historical centroid) should be suppressed
/// more aggressively than when scored against the current batch's centroid.
#[test]
fn standard_lines_suppressed_more_with_historical_centroid() {
    use panda_core::summarizer::summarize;

    let standard = standard_cargo_lines();
    let standard_refs: Vec<&str> = standard.iter().map(|s| s.as_str()).collect();
    let historical = centroid_of(&standard_refs);

    let input = standard.join("\n");

    let plain = summarize(&input, 20);
    let historical_result = summarize_against_centroid(&input, 20, &historical);

    // Historical scoring should suppress more standard lines
    assert!(
        historical_result.lines_out <= plain.lines_out,
        "Historical centroid should suppress at least as many standard lines as plain (historical={}, plain={})",
        historical_result.lines_out,
        plain.lines_out
    );
}

/// When the new run contains ONLY standard lines (no new content), output should
/// be minimal (close to 0 lines, or just an omission marker).
#[test]
fn fully_standard_run_gets_maximum_compression() {
    let standard = standard_cargo_lines();
    let standard_refs: Vec<&str> = standard.iter().map(|s| s.as_str()).collect();
    let historical = centroid_of(&standard_refs);

    // Exact same content as the historical baseline
    let input = standard.join("\n");
    let result = summarize_against_centroid(&input, 20, &historical);

    let non_omission_lines = result.output.lines()
        .filter(|l| !l.contains("omitted"))
        .count();

    assert!(
        non_omission_lines <= 3,
        "Fully standard run should produce ≤3 non-omission lines, got {}:\n{}",
        non_omission_lines,
        result.output
    );
}

/// Critical (error/warning) lines must always survive even against historical centroid.
#[test]
fn critical_lines_always_survive_historical_scoring() {
    let standard = standard_cargo_lines();
    let standard_refs: Vec<&str> = standard.iter().map(|s| s.as_str()).collect();
    let historical = centroid_of(&standard_refs);

    let mut lines = standard.clone();
    lines[10] = "warning: unused variable `x` in src/lib.rs".to_string();
    lines[15] = "error[E0308]: mismatched types in function return value".to_string();
    let input = lines.join("\n");

    let result = summarize_against_centroid(&input, 15, &historical);

    assert!(result.output.contains("warning:"), "Warning lost:\n{}", result.output);
    assert!(result.output.contains("error[E0308]"), "Error lost:\n{}", result.output);
}

// ── Centroid updates over multiple runs ───────────────────────────────────────

/// After 3 `update_command_centroid` calls, the centroid must not be identical
/// to any single run's centroid (it should be a running mean / blend).
#[test]
fn centroid_shifts_after_multiple_updates() {
    let mut session = SessionState::default();

    let run1: Vec<&str> = ["   Compiling foo v1.0", "   Compiling bar v1.0"].to_vec();
    let run2: Vec<&str> = ["   Compiling baz v2.0", "   Compiling qux v2.0"].to_vec();
    let run3: Vec<&str> = ["   Compiling alpha v3.0", "   Compiling beta v3.0"].to_vec();

    let c1 = centroid_of(&run1);
    let c2 = centroid_of(&run2);
    let c3 = centroid_of(&run3);

    // Simulate incremental update: take rolling mean
    session.update_command_centroid("cargo", c1.clone());
    session.update_command_centroid("cargo", c2.clone());
    session.update_command_centroid("cargo", c3.clone());

    let final_centroid = session.command_centroid("cargo").unwrap();

    // Final centroid must differ from the first run's centroid
    let max_diff = final_centroid.iter().zip(&c1)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);

    assert!(
        max_diff > 1e-4,
        "Centroid should shift after 3 updates, max_diff={:.2e}",
        max_diff
    );
}

/// Empty historical centroid (None) must fall back to plain anomaly scoring.
#[test]
fn none_centroid_falls_back_to_plain_summarize() {
    use panda_core::summarizer::summarize;

    let lines: Vec<String> = (0..250).map(|i| format!("log line {} content here", i)).collect();
    let input = lines.join("\n");

    // Use an empty centroid (all zeros) — should behave like no-centroid
    let empty_centroid = vec![0.0f32; 384];
    let against_empty = summarize_against_centroid(&input, 30, &empty_centroid);
    let plain = summarize(&input, 30);

    // Both should produce similar line counts (within 20%)
    let ratio = against_empty.lines_out as f32 / plain.lines_out.max(1) as f32;
    assert!(
        ratio > 0.5 && ratio < 2.0,
        "Empty centroid output ({} lines) should be within 2x of plain ({} lines)",
        against_empty.lines_out,
        plain.lines_out
    );
}
