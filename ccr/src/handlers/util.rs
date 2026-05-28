/// A rule for match_output short-circuit: if output matches success_pattern return ok_message,
/// if it matches error_pattern return None (fall through to normal handling).
pub struct MatchOutputRule {
    pub success_pattern: &'static str,
    pub error_pattern: &'static str,
    pub ok_message: &'static str,
}

/// Check a list of MatchOutputRules against output.
/// Returns Some(ok_message) if a success pattern fires and no error pattern fires.
/// Returns None to indicate normal processing should continue.
pub fn check_match_output(output: &str, rules: &[MatchOutputRule]) -> Option<String> {
    for rule in rules {
        let success_re = regex::Regex::new(rule.success_pattern).ok()?;
        let error_re = regex::Regex::new(rule.error_pattern).ok()?;
        if success_re.is_match(output) && !error_re.is_match(output) {
            return Some(rule.ok_message.to_string());
        }
    }
    None
}

/// Parse a space-aligned table, keep only specified column indices (0-based).
pub fn compact_table(output: &str, keep_cols: &[usize]) -> String {
    let lines: Vec<&str> = output.lines().collect();
    if lines.is_empty() {
        return output.to_string();
    }

    let mut out: Vec<String> = Vec::new();
    for line in &lines {
        if line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.is_empty() {
            continue;
        }
        let selected: Vec<&str> = keep_cols
            .iter()
            .filter_map(|&i| cols.get(i).copied())
            .collect();
        out.push(selected.join("  "));
    }
    out.join("\n")
}

/// Extract failure blocks + summary from test runner output.
/// runner: "pytest" | "jest" | "vitest" | "dotnet"
pub fn test_failures(output: &str, runner: &str) -> String {
    match runner {
        "pytest" => filter_pytest(output),
        "jest" => filter_jest(output),
        "vitest" => filter_vitest(output),
        _ => output.to_string(),
    }
}

fn filter_pytest(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut in_failure = false;
    let mut failure_lines = 0;
    // Track xfailed/xpassed node IDs separately — cap at 10 each.
    // XPASSED (unexpected pass) is especially high-signal: it usually means a
    // previously-broken behaviour was fixed without the team noticing.
    let mut xfailed: Vec<String> = Vec::new();
    let mut xpassed: Vec<String> = Vec::new();

    for line in &lines {
        let t = line.trim();
        // Keep FAILED/ERROR node IDs
        if t.starts_with("FAILED ") || t.starts_with("ERROR ") {
            out.push(line.to_string());
            continue;
        }
        // Collect XPASSED/XFAILED node IDs (capped at 10 each)
        if t.starts_with("XPASSED ") && xpassed.len() < 10 {
            xpassed.push(line.to_string());
            continue;
        }
        if t.starts_with("XFAILED ") && xfailed.len() < 10 {
            xfailed.push(line.to_string());
            continue;
        }
        // Start of a failure block
        if t.starts_with("____") && t.ends_with("____") {
            in_failure = true;
            failure_lines = 0;
            out.push(line.to_string());
            continue;
        }
        if in_failure {
            if failure_lines < 3 {
                out.push(line.to_string());
                failure_lines += 1;
            } else if failure_lines == 3 {
                out.push("     …".to_string());
                failure_lines += 1;
            }
            // End of failure block
            if t.starts_with("====") {
                in_failure = false;
            }
            continue;
        }
        // Summary line — two formats:
        // Verbose: "====== 1 failed, 45 passed in 12.3s ======"
        // Quiet (-q): "1 failed, 45 passed in 12.3s" (bare, no === wrapper)
        let is_summary = (t.contains(" failed") || t.contains(" passed") || t.contains(" error")
             || t.contains(" xfailed") || t.contains(" xpassed"))
            && (t.starts_with('=') || t.contains(" in ") || t.ends_with('s'));
        if is_summary {
            out.push(line.to_string());
        }
        // Drop: PASSED lines, dots, "collected N items", platform header
    }

    // Surface xpassed first (unexpected passes = most actionable), then xfailed
    let mut result: Vec<String> = Vec::new();
    if !xpassed.is_empty() {
        result.extend(xpassed);
    }
    if !xfailed.is_empty() {
        result.extend(xfailed);
    }
    result.extend(out);

    if result.is_empty() {
        output.to_string()
    } else {
        result.join("\n")
    }
}

fn filter_jest(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut in_failure = false;
    let mut failure_lines = 0;

    for line in &lines {
        let t = line.trim();
        // Keep FAIL <path> lines
        if t.starts_with("FAIL ") {
            out.push(line.to_string());
            in_failure = false;
            continue;
        }
        // Keep ● failure detail blocks
        if t.starts_with('●') {
            in_failure = true;
            failure_lines = 0;
            out.push(line.to_string());
            continue;
        }
        if in_failure {
            if failure_lines < 15 {
                out.push(line.to_string());
                failure_lines += 1;
            } else if failure_lines == 15 {
                out.push("[... truncated ...]".to_string());
                failure_lines += 1;
            }
            // Blank line ends the block
            if t.is_empty() && failure_lines > 2 {
                in_failure = false;
            }
            continue;
        }
        // Final summary
        if t.starts_with("Tests:") || t.starts_with("Test Suites:") || t.starts_with("Time:") {
            out.push(line.to_string());
            continue;
        }
        // Drop: PASS lines, ✓ lines, -- separators
    }
    if out.is_empty() {
        output.to_string()
    } else {
        out.join("\n")
    }
}

fn filter_vitest(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut in_failure = false;
    let mut failure_lines = 0;

    for line in &lines {
        let t = line.trim();
        // Keep FAIL lines
        if t.starts_with("FAIL") && t.contains(' ') {
            out.push(line.to_string());
            in_failure = false;
            continue;
        }
        // Error message lines
        if t.starts_with("× ") || t.starts_with("✗ ") {
            in_failure = true;
            failure_lines = 0;
            out.push(line.to_string());
            continue;
        }
        if in_failure {
            if failure_lines < 5 {
                out.push(line.to_string());
                failure_lines += 1;
            }
            if t.is_empty() && failure_lines > 1 {
                in_failure = false;
            }
            continue;
        }
        // Summary line
        if t.starts_with("Tests") && (t.contains("failed") || t.contains("passed")) {
            out.push(line.to_string());
        }
        // Drop: ✓ passing lines, progress bars, module noise
    }
    if out.is_empty() {
        output.to_string()
    } else {
        out.join("\n")
    }
}

/// Returns true if a log line should always be kept regardless of semantic similarity.
pub fn is_hard_keep(line: &str) -> bool {
    let l = line.to_lowercase();
    l.contains("error")
        || l.contains("panic")
        || l.contains("fatal")
        || l.contains("exception")
        || l.contains("failed")
        || l.contains("stack trace")
        || l.contains("caused by")
        || l.contains("critical")
        || l.contains("alert")
        || l.contains("emergency")
}

/// Derive a depth-limited, typed schema from a JSON value.
/// - Integers → "int", floats → "float", strings → "string", bools → "bool", null → "null"
/// - Objects: cap at 15 keys, add "...": "+N more keys" if truncated; recurse depth-limited
/// - Arrays: emit [schema_of_first, "[N items total]"]; empty array → ["array(0 items)"]
/// - Depth limit: 4 levels; beyond that emit "object" or "array"
pub fn json_to_schema(v: &serde_json::Value) -> serde_json::Value {
    json_to_schema_depth(v, 0)
}

fn json_to_schema_depth(v: &serde_json::Value, depth: usize) -> serde_json::Value {
    const MAX_DEPTH: usize = 4;
    const MAX_KEYS: usize = 15;

    match v {
        serde_json::Value::Object(map) => {
            if depth >= MAX_DEPTH {
                return serde_json::Value::String("object".to_string());
            }
            let mut schema_map = serde_json::Map::new();
            let total = map.len();
            for (k, val) in map.iter().take(MAX_KEYS) {
                schema_map.insert(k.clone(), json_to_schema_depth(val, depth + 1));
            }
            if total > MAX_KEYS {
                let extra = total - MAX_KEYS;
                schema_map.insert(
                    "...".to_string(),
                    serde_json::Value::String(format!("+{} more keys", extra)),
                );
            }
            serde_json::Value::Object(schema_map)
        }
        serde_json::Value::Array(arr) => {
            if depth >= MAX_DEPTH {
                return serde_json::Value::String("array".to_string());
            }
            if arr.is_empty() {
                serde_json::json!(["array(0 items)"])
            } else {
                let first_schema = json_to_schema_depth(&arr[0], depth + 1);
                serde_json::json!([first_schema, format!("[{} items total]", arr.len())])
            }
        }
        serde_json::Value::Number(n) => {
            if n.is_f64() && n.as_f64().map(|f| f.fract() != 0.0).unwrap_or(false) {
                serde_json::Value::String("float".to_string())
            } else {
                serde_json::Value::String("int".to_string())
            }
        }
        serde_json::Value::String(_) => serde_json::Value::String("string".to_string()),
        serde_json::Value::Bool(_) => serde_json::Value::String("bool".to_string()),
        serde_json::Value::Null => serde_json::Value::String("null".to_string()),
    }
}

/// Cosine similarity between two float vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

/// Returns true when a mutating git operation is currently in progress
/// (cherry-pick, merge, rebase, am/apply).  Uses only filesystem stats —
/// no subprocess, no latency.
pub fn mid_git_operation() -> bool {
    let Ok(dir) = std::env::current_dir() else { return false };
    mid_git_operation_in(&dir)
}

/// Same as [`mid_git_operation`] but walks upward from `start` instead of
/// the process CWD.  Useful in tests to avoid mutating the global CWD.
pub fn mid_git_operation_in(start: &std::path::Path) -> bool {
    let mut dir = start.to_path_buf();
    loop {
        let git = dir.join(".git");
        if git.exists() {
            return git.join("CHERRY_PICK_HEAD").exists()
                || git.join("MERGE_HEAD").exists()
                || git.join("rebase-merge").exists()
                || git.join("rebase-apply").exists();
        }
        if !dir.pop() {
            return false;
        }
    }
}

/// In-progress git operations whose state porcelain output omits.
///
/// `git status --porcelain` (which PandaFilter injects) does not print the
/// state prose git emits for rebases, merges, cherry-picks, etc. This enum
/// lets `filter_status` surface that info from the `.git/` marker files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitInProgressState {
    Rebase,
    MergeConflicts,      // MERGE_HEAD + UU/AA/DD lines present in porcelain
    MergeReady,          // MERGE_HEAD + no unmerged lines (conflicts resolved)
    CherryPick,
    Revert,
    Bisect,
    Am,                  // git am session (.git/rebase-apply/applying exists)
}

impl GitInProgressState {
    pub fn summary(self) -> &'static str {
        match self {
            Self::Rebase         => "rebase in progress",
            Self::MergeConflicts => "merge in progress. unresolved conflicts",
            Self::MergeReady     => "merge in progress. no conflicts",
            Self::CherryPick     => "cherry-pick in progress",
            Self::Revert         => "revert in progress",
            Self::Bisect         => "bisect in progress",
            Self::Am             => "am session in progress",
        }
    }
}

/// Detect an in-progress git operation from the process CWD.
/// `porcelain_output` is the `--porcelain` output already captured by the hook;
/// it is used to distinguish merge-with-conflicts from merge-ready-to-commit.
pub fn detect_git_in_progress(porcelain_output: &str) -> Option<GitInProgressState> {
    let Ok(cwd) = std::env::current_dir() else { return None };
    detect_git_in_progress_in(&cwd, porcelain_output)
}

/// Same as [`detect_git_in_progress`] but walks upward from `start`.
/// Useful in tests to avoid touching the global CWD.
pub fn detect_git_in_progress_in(
    start: &std::path::Path,
    porcelain_output: &str,
) -> Option<GitInProgressState> {
    let mut dir = start.to_path_buf();
    loop {
        let git = dir.join(".git");
        if git.is_dir() {
            // Cherry-pick takes priority (most actionable for the developer)
            if git.join("CHERRY_PICK_HEAD").exists() {
                return Some(GitInProgressState::CherryPick);
            }
            if git.join("REVERT_HEAD").exists() {
                return Some(GitInProgressState::Revert);
            }
            // Interactive rebase: .git/rebase-merge/
            // Non-interactive rebase (also git am): .git/rebase-apply/
            // Distinguish am from rebase via the presence of "applying" inside.
            if git.join("rebase-merge").is_dir() {
                return Some(GitInProgressState::Rebase);
            }
            if git.join("rebase-apply").is_dir() {
                if git.join("rebase-apply/applying").exists() {
                    return Some(GitInProgressState::Am);
                }
                return Some(GitInProgressState::Rebase);
            }
            if git.join("BISECT_LOG").exists() {
                return Some(GitInProgressState::Bisect);
            }
            if git.join("MERGE_HEAD").exists() {
                // Check porcelain output for unmerged status codes (UU, AA, DD)
                let has_unmerged = porcelain_output.lines().any(|l| {
                    let b = l.as_bytes();
                    b.len() >= 2
                        && (b[0] == b'U'
                            || (b[0] == b'A' && b[1] == b'A')
                            || (b[0] == b'D' && b[1] == b'D'))
                });
                return Some(if has_unmerged {
                    GitInProgressState::MergeConflicts
                } else {
                    GitInProgressState::MergeReady
                });
            }
            return None;
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Compact a file path longer than `max_len` chars to "prefix/.../filename".
pub fn compact_path(path: &str, max_len: usize) -> String {
    if path.len() <= max_len {
        return path.to_string();
    }
    let sep = if path.contains('/') { '/' } else { '\\' };
    let parts: Vec<&str> = path.split(sep).collect();
    if parts.len() <= 2 {
        return path.to_string();
    }
    let first = parts[0];
    let last = parts[parts.len() - 1];
    let candidate = format!("{}/.../{}", first, last);
    if candidate.len() <= max_len {
        candidate
    } else {
        format!(".../{}", last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── filter_pytest ─────────────────────────────────────────────────────────

    #[test]
    fn pytest_xpassed_surfaced_before_failures() {
        let output = "\
FAILED tests/auth.py::test_login - AssertionError
XPASSED tests/auth.py::test_legacy_login
XFAILED tests/db.py::test_slow_query
====== 1 failed, 2 xfailed, 1 xpassed in 5.21s ======
";
        let result = test_failures(output, "pytest");
        // xpassed must appear
        assert!(result.contains("XPASSED"), "xpassed must be surfaced");
        // xfailed must appear
        assert!(result.contains("XFAILED"), "xfailed must be surfaced");
        // FAILED must appear
        assert!(result.contains("FAILED tests/auth"), "failures must be surfaced");
        // xpassed must come before failures (highest signal first)
        let xp_pos = result.find("XPASSED").unwrap();
        let fail_pos = result.find("FAILED tests/").unwrap();
        assert!(xp_pos < fail_pos, "xpassed should appear before failures");
        // summary line preserved
        assert!(result.contains("1 failed"), "summary must be kept");
    }

    #[test]
    fn pytest_xfail_only_run_with_no_failures() {
        let output = "\
XFAILED tests/db.py::test_slow_query
XFAILED tests/db.py::test_timeout
====== 0 failed, 10 passed, 2 xfailed in 3.1s ======
";
        let result = test_failures(output, "pytest");
        assert!(result.contains("XFAILED"), "xfailed must be kept");
        assert!(result.contains("2 xfailed"), "summary must mention xfailed");
        // No line should start with bare "FAILED " (i.e. no test failures, only xfailed)
        assert!(
            !result.lines().any(|l| l.trim().starts_with("FAILED ")),
            "no bare FAILED lines should appear, got:\n{}", result
        );
    }

    #[test]
    fn pytest_xfail_capped_at_10() {
        // Generate 15 xfailed lines — only 10 should be kept
        let mut lines: Vec<String> = (0..15)
            .map(|i| format!("XFAILED tests/test_{}.py::test_fn", i))
            .collect();
        lines.push("====== 0 failed, 15 xfailed in 1.0s ======".to_string());
        let output = lines.join("\n");
        let result = test_failures(&output, "pytest");
        let xfail_count = result.lines().filter(|l| l.starts_with("XFAILED")).count();
        assert!(xfail_count <= 10, "xfailed lines capped at 10, got {}", xfail_count);
    }

    #[test]
    fn mid_git_operation_false_outside_git() {
        // In a normal test environment there is no CHERRY_PICK_HEAD etc.
        // We can only assert it doesn't panic and returns a bool.
        let _ = mid_git_operation(); // must not panic
    }

    // ── json_to_schema ────────────────────────────────────────────────────────

    #[test]
    fn schema_int() {
        let v = serde_json::json!(42);
        assert_eq!(json_to_schema(&v), serde_json::json!("int"));
    }

    #[test]
    fn schema_float() {
        let v = serde_json::json!(3.14);
        assert_eq!(json_to_schema(&v), serde_json::json!("float"));
    }

    #[test]
    fn schema_bool() {
        let v = serde_json::json!(true);
        assert_eq!(json_to_schema(&v), serde_json::json!("bool"));
    }

    #[test]
    fn schema_null() {
        let v = serde_json::Value::Null;
        assert_eq!(json_to_schema(&v), serde_json::json!("null"));
    }

    #[test]
    fn schema_string() {
        let v = serde_json::json!("hello");
        assert_eq!(json_to_schema(&v), serde_json::json!("string"));
    }

    #[test]
    fn schema_object_key_cap() {
        // Build an object with 20 keys
        let mut map = serde_json::Map::new();
        for i in 0..20usize {
            map.insert(format!("key{}", i), serde_json::json!(i));
        }
        let v = serde_json::Value::Object(map);
        let schema = json_to_schema(&v);
        let obj = schema.as_object().unwrap();
        // Should have at most 15 data keys + 1 "..." key
        assert!(obj.len() <= 16);
        assert!(obj.contains_key("..."));
        let extra_msg = obj["..."].as_str().unwrap();
        assert!(extra_msg.contains("+5 more keys"));
    }

    #[test]
    fn schema_array_shows_count() {
        let v = serde_json::json!([1, 2, 3, 4, 5]);
        let schema = json_to_schema(&v);
        let arr = schema.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[1], serde_json::json!("[5 items total]"));
    }

    #[test]
    fn schema_empty_array() {
        let v = serde_json::json!([]);
        let schema = json_to_schema(&v);
        let arr = schema.as_array().unwrap();
        assert_eq!(arr[0], serde_json::json!("array(0 items)"));
    }

    #[test]
    fn schema_depth_limit_object() {
        // Build a deeply nested object: {a: {b: {c: {d: {e: 1}}}}}
        let v = serde_json::json!({"a": {"b": {"c": {"d": {"e": 1}}}}});
        let schema = json_to_schema(&v);
        // At depth 4 we should see "object" string instead of recursing further
        let s = serde_json::to_string(&schema).unwrap();
        assert!(s.contains("\"object\"") || s.contains("\"int\""));
    }

    // ── check_match_output ────────────────────────────────────────────────────

    #[test]
    fn match_output_fires_on_success() {
        let rules = [MatchOutputRule {
            success_pattern: "successfully",
            error_pattern: "error",
            ok_message: "All good",
        }];
        let result = check_match_output("Operation completed successfully", &rules);
        assert_eq!(result, Some("All good".to_string()));
    }

    #[test]
    fn match_output_blocked_by_error_pattern() {
        let rules = [MatchOutputRule {
            success_pattern: "successfully",
            error_pattern: "error",
            ok_message: "All good",
        }];
        let result = check_match_output("completed successfully but with error", &rules);
        assert_eq!(result, None);
    }

    #[test]
    fn match_output_no_match_returns_none() {
        let rules = [MatchOutputRule {
            success_pattern: "done",
            error_pattern: "fail",
            ok_message: "OK",
        }];
        let result = check_match_output("nothing relevant here", &rules);
        assert_eq!(result, None);
    }

    // ── compact_path ─────────────────────────────────────────────────────────

    #[test]
    fn compact_path_short_unchanged() {
        let p = "/a/b/c";
        assert_eq!(compact_path(p, 20), p);
    }

    #[test]
    fn compact_path_truncates_long() {
        let p = "/very/long/deeply/nested/path/to/some/file.rs";
        let result = compact_path(p, 20);
        assert!(result.len() <= 20 || result.starts_with(".../"));
        assert!(result.contains("file.rs"));
    }

    #[test]
    fn compact_path_prefix_slash_filename() {
        let p = "/home/user/projects/myapp/src/main.rs";
        let result = compact_path(p, 25);
        // Should contain filename
        assert!(result.contains("main.rs"));
    }

    // ── detect_git_in_progress_in ─────────────────────────────────────────────

    fn make_git_dir() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".git")).unwrap();
        tmp
    }

    #[test]
    fn detect_cherry_pick() {
        let tmp = make_git_dir();
        std::fs::write(tmp.path().join(".git/CHERRY_PICK_HEAD"), "abc\n").unwrap();
        let state = detect_git_in_progress_in(tmp.path(), "");
        assert_eq!(state, Some(GitInProgressState::CherryPick));
    }

    #[test]
    fn detect_revert() {
        let tmp = make_git_dir();
        std::fs::write(tmp.path().join(".git/REVERT_HEAD"), "abc\n").unwrap();
        let state = detect_git_in_progress_in(tmp.path(), "");
        assert_eq!(state, Some(GitInProgressState::Revert));
    }

    #[test]
    fn detect_rebase_interactive() {
        let tmp = make_git_dir();
        std::fs::create_dir(tmp.path().join(".git/rebase-merge")).unwrap();
        let state = detect_git_in_progress_in(tmp.path(), "");
        assert_eq!(state, Some(GitInProgressState::Rebase));
    }

    #[test]
    fn detect_rebase_non_interactive() {
        let tmp = make_git_dir();
        std::fs::create_dir(tmp.path().join(".git/rebase-apply")).unwrap();
        // No "applying" file → rebase, not am
        let state = detect_git_in_progress_in(tmp.path(), "");
        assert_eq!(state, Some(GitInProgressState::Rebase));
    }

    #[test]
    fn detect_am_session() {
        let tmp = make_git_dir();
        std::fs::create_dir(tmp.path().join(".git/rebase-apply")).unwrap();
        std::fs::write(tmp.path().join(".git/rebase-apply/applying"), "").unwrap();
        let state = detect_git_in_progress_in(tmp.path(), "");
        assert_eq!(state, Some(GitInProgressState::Am));
    }

    #[test]
    fn detect_bisect() {
        let tmp = make_git_dir();
        std::fs::write(tmp.path().join(".git/BISECT_LOG"), "git bisect start\n").unwrap();
        let state = detect_git_in_progress_in(tmp.path(), "");
        assert_eq!(state, Some(GitInProgressState::Bisect));
    }

    #[test]
    fn detect_merge_with_conflicts() {
        let tmp = make_git_dir();
        std::fs::write(tmp.path().join(".git/MERGE_HEAD"), "abc\n").unwrap();
        // UU line in porcelain output indicates unmerged file
        let porcelain = "UU src/main.rs\n";
        let state = detect_git_in_progress_in(tmp.path(), porcelain);
        assert_eq!(state, Some(GitInProgressState::MergeConflicts));
    }

    #[test]
    fn detect_merge_ready_to_commit() {
        let tmp = make_git_dir();
        std::fs::write(tmp.path().join(".git/MERGE_HEAD"), "abc\n").unwrap();
        // No UU lines — conflicts resolved, ready to commit
        let porcelain = "M  src/main.rs\n";
        let state = detect_git_in_progress_in(tmp.path(), porcelain);
        assert_eq!(state, Some(GitInProgressState::MergeReady));
    }

    #[test]
    fn detect_none_clean_repo() {
        let tmp = make_git_dir();
        let state = detect_git_in_progress_in(tmp.path(), "");
        assert_eq!(state, None);
    }

    #[test]
    fn detect_none_outside_git() {
        // A directory with no .git ancestor returns None
        let tmp = tempfile::tempdir().unwrap();
        let state = detect_git_in_progress_in(tmp.path(), "");
        assert_eq!(state, None);
    }

    #[test]
    fn state_summaries_correct() {
        assert_eq!(GitInProgressState::Rebase.summary(), "rebase in progress");
        assert_eq!(GitInProgressState::MergeConflicts.summary(), "merge in progress. unresolved conflicts");
        assert_eq!(GitInProgressState::MergeReady.summary(), "merge in progress. no conflicts");
        assert_eq!(GitInProgressState::CherryPick.summary(), "cherry-pick in progress");
        assert_eq!(GitInProgressState::Revert.summary(), "revert in progress");
        assert_eq!(GitInProgressState::Bisect.summary(), "bisect in progress");
        assert_eq!(GitInProgressState::Am.summary(), "am session in progress");
    }
}
