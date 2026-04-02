//! NDJSON (newline-delimited JSON) streaming compaction.
//!
//! Detects and compresses outputs like `go test -json` and jest JSON reporter.
//! Only fires when `detect()` returns true, so it is zero-cost for non-NDJSON output.

use serde_json::Value;
use std::collections::BTreeMap;
use std::collections::HashSet;

// ── Public API ────────────────────────────────────────────────────────────────

/// Returns `true` if ≥3 of the first 10 non-empty lines are valid JSON objects.
pub fn detect(input: &str) -> bool {
    let matching = input
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(10)
        .filter(|l| {
            let t = l.trim();
            t.starts_with('{')
                && t.ends_with('}')
                && serde_json::from_str::<Value>(t).is_ok()
        })
        .count();
    matching >= 3
}

/// Dispatch to the appropriate compactor based on `hint`.
pub fn compact(input: &str, hint: &str) -> String {
    let h = hint.to_lowercase();
    if h.contains("go test") || h == "go" || h.starts_with("go ") {
        compact_go_test(input)
    } else if h.contains("jest") || h.contains("vitest") {
        compact_jest_json(input)
    } else if h.contains("cargo") {
        compact_cargo_json(input)
    } else {
        compact_generic(input)
    }
}

// ── go test -json ─────────────────────────────────────────────────────────────

/// Compact the streaming NDJSON output of `go test -json`.
///
/// Groups by package; shows failures with their output; suppresses PASS lines.
pub fn compact_go_test(input: &str) -> String {
    #[derive(Default)]
    struct PkgState {
        failed: bool,
        /// Lines accumulated in "output" actions (filtered to exclude pass/run markers).
        output_lines: Vec<String>,
        elapsed: Option<f64>,
    }

    let mut packages: BTreeMap<String, PkgState> = BTreeMap::new();
    let mut total_pkgs = 0usize;
    let mut failed_pkgs = 0usize;

    for line in input.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(t) else {
            continue;
        };

        let action = v["Action"].as_str().unwrap_or("");
        let pkg = v["Package"].as_str().unwrap_or("").to_string();
        let test = v["Test"].as_str();

        match action {
            "output" => {
                if let Some(out_str) = v["Output"].as_str() {
                    let out_str = out_str.trim_end_matches('\n');
                    // Suppress pure noise lines
                    if !out_str.starts_with("--- PASS")
                        && !out_str.starts_with("=== RUN")
                        && !out_str.starts_with("=== CONT")
                        && !out_str.starts_with("=== PAUSE")
                        && !out_str.trim().is_empty()
                    {
                        let state = packages.entry(pkg).or_default();
                        if state.output_lines.len() < 100 {
                            state.output_lines.push(out_str.to_string());
                        }
                    }
                }
            }
            "pass" => {
                if test.is_none() {
                    // Package-level pass
                    total_pkgs += 1;
                    let state = packages.entry(pkg).or_default();
                    state.elapsed = v["Elapsed"].as_f64();
                    // Clear output on package pass (tests passed — output is noise)
                    state.output_lines.clear();
                }
            }
            "fail" => {
                if test.is_none() {
                    // Package-level fail
                    total_pkgs += 1;
                    failed_pkgs += 1;
                    let state = packages.entry(pkg).or_default();
                    state.failed = true;
                    state.elapsed = v["Elapsed"].as_f64();
                }
            }
            _ => {}
        }
    }

    let mut out: Vec<String> = Vec::new();

    for (pkg, state) in &packages {
        if state.failed {
            out.push(format!("FAIL {}", pkg));
            for line in &state.output_lines {
                if !line.trim().is_empty() {
                    out.push(format!("  {}", line));
                }
            }
        } else {
            let elapsed = state
                .elapsed
                .map(|e| format!(" ({:.3}s)", e))
                .unwrap_or_default();
            out.push(format!("ok {}{}", pkg, elapsed));
        }
    }

    let passed = total_pkgs.saturating_sub(failed_pkgs);
    if failed_pkgs > 0 {
        out.push(format!(
            "FAIL: {}/{} packages failed",
            failed_pkgs, total_pkgs
        ));
    } else if total_pkgs > 0 {
        out.push(format!("ok {} package(s)", passed));
    }

    if out.is_empty() {
        input.to_string()
    } else {
        out.join("\n")
    }
}

// ── jest JSON streaming ───────────────────────────────────────────────────────

/// Compact jest's JSON-per-line streaming output.
pub fn compact_jest_json(input: &str) -> String {
    let mut failures: Vec<String> = Vec::new();
    let mut total_tests = 0usize;
    let mut failed_tests = 0usize;

    for line in input.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(t) else {
            continue;
        };

        if let Some(status) = v["status"].as_str() {
            total_tests += 1;
            if status == "failed" {
                failed_tests += 1;
                let title = v["ancestorTitles"]
                    .as_array()
                    .and_then(|a| a.last())
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let test_name = v["title"].as_str().unwrap_or("unknown");
                let full_name = if title.is_empty() {
                    test_name.to_string()
                } else {
                    format!("{} > {}", title, test_name)
                };
                failures.push(format!("FAIL: {}", full_name));

                if let Some(msgs) = v["failureMessages"].as_array() {
                    for m in msgs.iter().take(3) {
                        if let Some(s) = m.as_str() {
                            for l in s.lines().take(5) {
                                if !l.trim().is_empty() {
                                    failures.push(format!("  {}", l));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if failures.is_empty() {
        if total_tests > 0 {
            return format!("ok {} tests", total_tests);
        }
        return compact_generic(input);
    }

    let mut out = failures;
    out.push(format!(
        "{} failed, {} passed",
        failed_tests,
        total_tests.saturating_sub(failed_tests)
    ));
    out.join("\n")
}

// ── cargo --message-format json ───────────────────────────────────────────────

/// Compact `cargo --message-format json` output.
pub fn compact_cargo_json(input: &str) -> String {
    let mut errors: Vec<String> = Vec::new();
    let mut warnings = 0usize;

    for line in input.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(t) else {
            continue;
        };

        if v["reason"].as_str() != Some("compiler-message") {
            continue;
        }

        if let Some(msg) = v.get("message") {
            let level = msg["level"].as_str().unwrap_or("");
            let text = msg["message"].as_str().unwrap_or("");
            match level {
                "error" => {
                    errors.push(format!("error: {}", text));
                    if let Some(spans) = msg["spans"].as_array() {
                        for span in spans.iter().take(2) {
                            let file = span["file_name"].as_str().unwrap_or("");
                            let line_no = span["line_start"].as_u64().unwrap_or(0);
                            if !file.is_empty() {
                                errors.push(format!("  --> {}:{}", file, line_no));
                            }
                        }
                    }
                }
                "warning" => warnings += 1,
                _ => {}
            }
        }
    }

    if errors.is_empty() && warnings == 0 {
        return input.to_string();
    }

    let mut out = errors;
    if warnings > 0 {
        out.push(format!("[{} warning(s) suppressed]", warnings));
    }
    out.join("\n")
}

// ── Generic NDJSON ────────────────────────────────────────────────────────────

/// For unknown NDJSON: extract `message`/`msg`/`error`/`level` fields.
/// Groups by level (error > warn > info) and deduplicates repeated messages.
pub fn compact_generic(input: &str) -> String {
    let mut errors: Vec<String> = Vec::new();
    let mut warns: Vec<String> = Vec::new();
    let mut infos: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for line in input.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(t) else {
            continue;
        };

        let level = v["level"]
            .as_str()
            .or_else(|| v["severity"].as_str())
            .unwrap_or("info");
        let message = v["message"]
            .as_str()
            .or_else(|| v["msg"].as_str())
            .or_else(|| v["error"].as_str())
            .unwrap_or_default();

        if message.is_empty() {
            continue;
        }

        let key = format!("{}:{}", level, message);
        if !seen.insert(key) {
            continue; // deduplicate
        }

        let entry = format!("[{}] {}", level, message);
        match level {
            "error" | "fatal" | "critical" => errors.push(entry),
            "warn" | "warning" => warns.push(entry),
            _ => infos.push(entry),
        }
    }

    let mut out: Vec<String> = Vec::new();
    out.extend(errors);
    out.extend(warns);
    out.extend(infos.into_iter().take(10));

    if out.is_empty() {
        input.to_string()
    } else {
        out.join("\n")
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_true_for_go_test_json() {
        let input = r#"{"Action":"run","Package":"pkg","Test":"TestFoo"}
{"Action":"output","Package":"pkg","Test":"TestFoo","Output":"=== RUN TestFoo\n"}
{"Action":"pass","Package":"pkg","Test":"TestFoo","Elapsed":0.001}
{"Action":"pass","Package":"pkg","Elapsed":0.5}
"#;
        assert!(detect(input));
    }

    #[test]
    fn detect_false_for_plain_text() {
        let input = "error: build failed\nfoo.rs:42: expected token\nok\n";
        assert!(!detect(input));
    }

    #[test]
    fn detect_false_for_sparse_json() {
        // Only 1 JSON line out of 5 non-empty
        let input = r#"plain text
more text
{"Action":"pass"}
still text
and more"#;
        assert!(!detect(input));
    }

    #[test]
    fn go_test_all_pass_compact() {
        let input = r#"{"Action":"run","Package":"mypkg","Test":"TestA"}
{"Action":"output","Package":"mypkg","Test":"TestA","Output":"=== RUN TestA\n"}
{"Action":"pass","Package":"mypkg","Test":"TestA","Elapsed":0.001}
{"Action":"pass","Package":"mypkg","Elapsed":0.5}
"#;
        let result = compact_go_test(input);
        assert!(result.contains("ok mypkg"));
        assert!(result.contains("1 package"));
    }

    #[test]
    fn go_test_failure_preserved() {
        let input = r#"{"Action":"run","Package":"mypkg","Test":"TestBad"}
{"Action":"output","Package":"mypkg","Test":"TestBad","Output":"--- FAIL: TestBad\n"}
{"Action":"output","Package":"mypkg","Test":"TestBad","Output":"    expected 1, got 2\n"}
{"Action":"fail","Package":"mypkg","Test":"TestBad","Elapsed":0.001}
{"Action":"fail","Package":"mypkg","Elapsed":0.5}
"#;
        let result = compact_go_test(input);
        assert!(result.contains("FAIL mypkg"));
        assert!(result.contains("expected 1, got 2"));
    }

    #[test]
    fn go_test_pass_output_suppressed() {
        let input = r#"{"Action":"output","Package":"pkg","Test":"TestOk","Output":"=== RUN TestOk\n"}
{"Action":"pass","Package":"pkg","Test":"TestOk","Elapsed":0.001}
{"Action":"pass","Package":"pkg","Elapsed":1.0}
"#;
        let result = compact_go_test(input);
        assert!(!result.contains("=== RUN"), "RUN lines should be suppressed");
        assert!(result.contains("ok pkg"));
    }

    #[test]
    fn jest_failures_extracted() {
        let input = r#"{"status":"failed","ancestorTitles":["MyComponent"],"title":"renders correctly","failureMessages":["Expected: true\nReceived: false"]}
{"status":"passed","ancestorTitles":["MyComponent"],"title":"is accessible"}
{"status":"passed","ancestorTitles":["OtherSuite"],"title":"works"}
"#;
        let result = compact_jest_json(input);
        assert!(result.contains("FAIL"));
        assert!(result.contains("renders correctly"));
        assert!(result.contains("Expected: true"));
    }

    #[test]
    fn jest_all_pass_compact() {
        let input = r#"{"status":"passed","ancestorTitles":[],"title":"TestA"}
{"status":"passed","ancestorTitles":[],"title":"TestB"}
{"status":"passed","ancestorTitles":[],"title":"TestC"}
"#;
        let result = compact_jest_json(input);
        assert!(result.contains("ok 3 tests"));
    }

    #[test]
    fn generic_level_grouping() {
        let input = r#"{"level":"info","message":"starting up"}
{"level":"warn","message":"deprecated API"}
{"level":"error","message":"connection refused"}
{"level":"info","message":"retrying"}
"#;
        let result = compact_generic(input);
        let lines: Vec<&str> = result.lines().collect();
        // errors should come first
        assert!(lines[0].contains("[error]"));
        assert!(result.contains("[warn]"));
        assert!(result.contains("[info]"));
    }

    #[test]
    fn generic_deduplicates_repeated_messages() {
        let input = r#"{"level":"error","message":"disk full"}
{"level":"error","message":"disk full"}
{"level":"error","message":"disk full"}
"#;
        let result = compact_generic(input);
        let count = result.matches("disk full").count();
        assert_eq!(count, 1, "duplicate messages should be collapsed to one");
    }
}
