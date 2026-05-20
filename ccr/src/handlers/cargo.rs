use std::sync::OnceLock;

use super::Handler;

pub struct CargoHandler;

struct AggregatedTestResult {
    passed: usize,
    failed: usize,
    ignored: usize,
    filtered_out: usize,
    suites: usize,
    duration: Option<f64>,
}

impl AggregatedTestResult {
    fn new() -> Self {
        Self { passed: 0, failed: 0, ignored: 0, filtered_out: 0, suites: 0, duration: None }
    }

    fn parse_and_merge(&mut self, line: &str) -> bool {
        let re = re_test_result();
        if let Some(cap) = re.captures(line) {
            let status = cap.get(1).map(|m| m.as_str()).unwrap_or("");
            if status != "ok" && status != "FAILED" {
                return false;
            }
            self.passed += cap.get(2).and_then(|m| m.as_str().parse().ok()).unwrap_or(0usize);
            self.failed += cap.get(3).and_then(|m| m.as_str().parse().ok()).unwrap_or(0usize);
            self.ignored += cap.get(4).and_then(|m| m.as_str().parse().ok()).unwrap_or(0usize);
            self.filtered_out += cap.get(6).and_then(|m| m.as_str().parse().ok()).unwrap_or(0usize);
            if let Some(d) = cap.get(7).and_then(|m| m.as_str().parse::<f64>().ok()) {
                self.duration = Some(self.duration.map_or(d, |prev| if d > prev { d } else { prev }));
            }
            self.suites += 1;
            true
        } else {
            false
        }
    }

    fn format_compact(&self) -> String {
        let mut parts = vec![format!("{} passed", self.passed)];
        if self.failed > 0 {
            parts.push(format!("{} failed", self.failed));
        }
        if self.ignored > 0 {
            parts.push(format!("{} ignored", self.ignored));
        }
        if self.filtered_out > 0 {
            parts.push(format!("{} filtered", self.filtered_out));
        }
        let meta = match (self.suites, self.duration) {
            (s, Some(d)) => format!(" ({} suite{}, {:.2}s)", s, if s != 1 { "s" } else { "" }, d),
            (s, None) if s > 1 => format!(" ({} suites)", s),
            _ => String::new(),
        };
        format!("cargo test: {}{}", parts.join(", "), meta)
    }
}

fn re_test_result() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(
            r"test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out(?:; finished in ([\d.]+)s)?"
        ).expect("test result regex")
    })
}

/// Find the cargo subcommand, skipping any toolchain override token (`+nightly`, `+stable`, etc.)
/// that cargo allows between `cargo` and the subcommand.
///
/// Examples:
/// - `["cargo", "build"]`           → "build"
/// - `["cargo", "+nightly", "build"]`→ "build"
/// - `["cargo", "+1.70.0", "clippy"]`→ "clippy"
fn cargo_subcmd(args: &[String]) -> &str {
    for a in args.iter().skip(1) {
        if a.starts_with('+') {
            continue; // toolchain override: +nightly, +stable, +1.70.0, etc.
        }
        return a.as_str();
    }
    ""
}

fn re_clippy_rule() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\[(\w+)\]").expect("cargo clippy rule regex"))
}

impl Handler for CargoHandler {
    fn rewrite_args(&self, args: &[String]) -> Vec<String> {
        let subcmd = cargo_subcmd(args);
        match subcmd {
            "build" | "check" | "clippy" => {
                // Inject --message-format json unless already present.
                // Insert before any `--` separator so the flag is parsed by cargo,
                // not passed through to the underlying tool (e.g. clippy lints).
                if args.iter().any(|a| a.starts_with("--message-format")) {
                    args.to_vec()
                } else {
                    let mut out = Vec::with_capacity(args.len() + 2);
                    let mut inserted = false;
                    for a in args {
                        if a == "--" && !inserted {
                            out.push("--message-format".to_string());
                            out.push("json".to_string());
                            inserted = true;
                        }
                        out.push(a.clone());
                    }
                    if !inserted {
                        out.push("--message-format".to_string());
                        out.push("json".to_string());
                    }
                    out
                }
            }
            _ => args.to_vec(),
        }
    }

    fn filter(&self, output: &str, args: &[String]) -> String {
        let subcmd = cargo_subcmd(args);
        match subcmd {
            "build" | "check" | "clippy" => filter_build(output),
            "test" => filter_test(output),
            "nextest" => {
                if args.iter().any(|a| a == "run") {
                    filter_nextest(output)
                } else {
                    output.to_string()
                }
            }
            _ => output.to_string(),
        }
    }
}

/// Group clippy warnings by lint rule name (e.g. `[unused_variables]`).
/// Returns formatted lines: `[rule_name ×N]` plus up to 3 example location lines.
/// Only applied when there are 3 or more warnings.
fn group_clippy_warnings(warnings: &[String]) -> Vec<String> {
    if warnings.len() < 3 {
        return warnings.iter().map(|w| format!("  {}", w)).collect();
    }

    // Collect (rule_name, original_warning_line) pairs; ungrouped warnings kept as-is.
    let mut grouped: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    let mut ungrouped: Vec<String> = Vec::new();

    for w in warnings {
        if let Some(cap) = re_clippy_rule().captures(w) {
            let rule = cap[1].to_string();
            grouped.entry(rule).or_default().push(w.clone());
        } else {
            ungrouped.push(w.clone());
        }
    }

    let mut out: Vec<String> = Vec::new();

    for (rule, lines) in &grouped {
        out.push(format!("[{} \u{d7}{}]", rule, lines.len()));
        for loc in lines.iter().take(3) {
            // Extract location part: text after last `]` or the full line
            let location = re_clippy_rule()
                .find(loc)
                .map(|m: regex::Match| loc[m.end()..].trim())
                .unwrap_or(loc.trim());
            if !location.is_empty() {
                out.push(format!("    {}", location));
            }
        }
    }

    for w in &ungrouped {
        out.push(format!("  {}", w));
    }

    out
}

/// Filter `cargo build/check/clippy --message-format json` output.
/// Keeps only compiler-message (errors + warnings); discards compiler-artifact noise.
fn filter_build(output: &str) -> String {
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut success: Option<bool> = None;

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Try JSON parse first
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
            match v.get("reason").and_then(|r| r.as_str()) {
                Some("compiler-message") => {
                    if let Some(msg) = v.get("message") {
                        let level = msg.get("level").and_then(|l| l.as_str()).unwrap_or("");
                        let text = msg.get("message").and_then(|m| m.as_str()).unwrap_or("");
                        let location = msg
                            .get("spans")
                            .and_then(|s| s.as_array())
                            .and_then(|s| s.first())
                            .map(|span| {
                                let file =
                                    span.get("file_name").and_then(|f| f.as_str()).unwrap_or("");
                                let line_n =
                                    span.get("line_start").and_then(|l| l.as_u64()).unwrap_or(0);
                                format!(" [{}:{}]", file, line_n)
                            })
                            .unwrap_or_default();

                        match level {
                            "error" | "error[E]" => {
                                errors.push(format!("error: {}{}", text, location));
                            }
                            "warning" => {
                                warnings.push(format!("warning: {}{}", text, location));
                            }
                            _ => {}
                        }
                    }
                }
                Some("build-finished") => {
                    success = v.get("success").and_then(|s| s.as_bool());
                }
                _ => {}
            }
        } else {
            // Non-JSON line (e.g. cargo stderr without JSON flag, or mixed output)
            // Keep error/warning lines
            if trimmed.starts_with("error") || trimmed.starts_with("warning") {
                if trimmed.starts_with("error") {
                    errors.push(trimmed.to_string());
                } else {
                    warnings.push(trimmed.to_string());
                }
            }
        }
    }

    const MAX_ERRORS: usize = 15;
    let mut out: Vec<String> = Vec::new();
    let shown = errors.len().min(MAX_ERRORS);
    out.extend(errors[..shown].iter().cloned());
    if errors.len() > MAX_ERRORS {
        out.push(format!("[+{} more errors]", errors.len() - MAX_ERRORS));
    }
    if !warnings.is_empty() {
        out.push(format!("[{} warnings]", warnings.len()));
        let grouped = group_clippy_warnings(&warnings);
        out.extend(grouped);
    }
    match success {
        Some(true) => {
            if out.is_empty() {
                out.push("Build OK".to_string());
            }
        }
        Some(false) => {
            if errors.is_empty() {
                out.push("Build FAILED".to_string());
            }
        }
        None => {}
    }

    if out.is_empty() {
        output.to_string()
    } else {
        out.join("\n")
    }
}

/// Filter `cargo test` standard output.
/// All-pass: aggregates suite results into a compact one-liner.
/// Failures: shows up to 10 failures with truncated detail blocks.
fn filter_test(output: &str) -> String {
    let mut summary_lines: Vec<String> = Vec::new();
    let mut in_failure_detail = false;
    let mut failure_names: Vec<String> = Vec::new();
    let mut current_failure: Vec<String> = Vec::new();
    let mut failure_blocks: Vec<String> = Vec::new();

    for line in output.lines() {
        if line.trim_start().starts_with("test ") && line.ends_with("FAILED") {
            let name = line
                .trim_start()
                .trim_start_matches("test ")
                .trim_end_matches(" ... FAILED")
                .to_string();
            failure_names.push(name);
        }

        if line.starts_with("test result:") {
            summary_lines.push(line.to_string());
            in_failure_detail = false;
        }

        if line.starts_with("failures:") {
            in_failure_detail = true;
            continue;
        }
        if in_failure_detail {
            if line.trim().is_empty() && !current_failure.is_empty() {
                failure_blocks.push(current_failure.join("\n"));
                current_failure.clear();
            } else if !line.trim().is_empty() && !line.starts_with("test result:") {
                current_failure.push(line.to_string());
            }
        }
    }
    if !current_failure.is_empty() {
        failure_blocks.push(current_failure.join("\n"));
    }

    if failure_names.is_empty() {
        let mut agg = AggregatedTestResult::new();
        let mut any_parsed = false;
        for s in &summary_lines {
            if agg.parse_and_merge(s) {
                any_parsed = true;
            }
        }
        if any_parsed {
            return agg.format_compact();
        }
        return summary_lines.last().cloned().unwrap_or_else(|| output.to_string());
    }

    const MAX_FAILURES: usize = 10;
    const FAILURE_CHAR_CAP: usize = 200;
    let mut out: Vec<String> = Vec::new();
    let shown = failure_names.len().min(MAX_FAILURES);
    for name in &failure_names[..shown] {
        out.push(format!("FAILED: {}", name));
    }
    if failure_names.len() > MAX_FAILURES {
        out.push(format!("[+{} more failures]", failure_names.len() - MAX_FAILURES));
    }

    for block in failure_blocks.iter().take(MAX_FAILURES) {
        let chars: Vec<char> = block.chars().collect();
        if chars.len() > FAILURE_CHAR_CAP {
            out.push(chars[..FAILURE_CHAR_CAP].iter().collect::<String>() + "…");
        } else {
            out.push(block.clone());
        }
    }

    let mut agg = AggregatedTestResult::new();
    let mut any_parsed = false;
    for s in &summary_lines {
        if agg.parse_and_merge(s) {
            any_parsed = true;
        }
    }
    if any_parsed {
        out.push(agg.format_compact());
    } else if let Some(s) = summary_lines.last() {
        out.push(s.clone());
    }

    out.join("\n")
}

/// Filter `cargo nextest run` output.
/// Keeps FAIL lines and the Summary line; drops PASS and "running N tests" lines.
fn filter_nextest(output: &str) -> String {
    let mut failures: Vec<String> = Vec::new();
    let mut summary: Option<String> = None;
    let mut found_any = false;

    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if t.starts_with("FAIL") {
            failures.push(line.to_string());
            found_any = true;
        } else if t.starts_with("Summary") {
            summary = Some(line.to_string());
            found_any = true;
        }
    }

    if !found_any {
        return output.to_string();
    }

    let mut out: Vec<String> = failures;
    if let Some(s) = summary {
        out.push(s);
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── group_clippy_warnings ────────────────────────────────────────────────

    #[test]
    fn group_same_rule_five_warnings() {
        let warnings: Vec<String> = (1..=5)
            .map(|i| {
                format!(
                    "warning: unused variable [unused_variables] [src/main.rs:{}]",
                    i
                )
            })
            .collect();
        let result = group_clippy_warnings(&warnings);
        // First line should be the grouped header
        assert!(result[0].contains("unused_variables") && result[0].contains("×5"));
    }

    #[test]
    fn group_different_rules_grouped_separately() {
        let warnings = vec![
            "warning: unused variable `x` [unused_variables] [src/a.rs:1]".to_string(),
            "warning: unused variable `y` [unused_variables] [src/a.rs:2]".to_string(),
            "warning: function is never used: `foo` [dead_code] [src/b.rs:10]".to_string(),
            "warning: function is never used: `bar` [dead_code] [src/b.rs:20]".to_string(),
            "warning: function is never used: `baz` [dead_code] [src/b.rs:30]".to_string(),
        ];
        let result = group_clippy_warnings(&warnings);
        let output = result.join("\n");
        assert!(output.contains("dead_code") && output.contains("×3"));
        assert!(output.contains("unused_variables") && output.contains("×2"));
    }

    // ── rewrite_args ─────────────────────────────────────────────────────

    #[test]
    fn message_format_injected_before_separator() {
        let handler = CargoHandler;
        let args: Vec<String> = vec!["cargo", "clippy", "--", "-D", "warnings"]
            .into_iter().map(String::from).collect();
        let result = handler.rewrite_args(&args);
        let sep_pos = result.iter().position(|a| a == "--").unwrap();
        let fmt_pos = result.iter().position(|a| a == "--message-format").unwrap();
        assert!(fmt_pos < sep_pos, "--message-format must come before --");
    }

    #[test]
    fn message_format_appended_when_no_separator() {
        let handler = CargoHandler;
        let args: Vec<String> = vec!["cargo", "build"]
            .into_iter().map(String::from).collect();
        let result = handler.rewrite_args(&args);
        assert!(result.contains(&"--message-format".to_string()));
        assert!(result.contains(&"json".to_string()));
    }

    #[test]
    fn message_format_not_doubled() {
        let handler = CargoHandler;
        let args: Vec<String> = vec!["cargo", "check", "--message-format", "json"]
            .into_iter().map(String::from).collect();
        let result = handler.rewrite_args(&args);
        let count = result.iter().filter(|a| a.as_str() == "--message-format").count();
        assert_eq!(count, 1, "should not inject a second --message-format");
    }

    #[test]
    fn message_format_only_before_first_separator() {
        let handler = CargoHandler;
        let args: Vec<String> = vec!["cargo", "clippy", "--", "-D", "warnings", "--", "extra"]
            .into_iter().map(String::from).collect();
        let result = handler.rewrite_args(&args);
        let fmt_count = result.iter().filter(|a| a.as_str() == "--message-format").count();
        assert_eq!(fmt_count, 1, "should only inject once even with multiple --");
        let sep_pos = result.iter().position(|a| a == "--").unwrap();
        let fmt_pos = result.iter().position(|a| a == "--message-format").unwrap();
        assert!(fmt_pos < sep_pos);
    }

    #[test]
    fn non_build_subcommand_not_injected() {
        let handler = CargoHandler;
        let args: Vec<String> = vec!["cargo", "test", "--", "--nocapture"]
            .into_iter().map(String::from).collect();
        let result = handler.rewrite_args(&args);
        assert!(!result.contains(&"--message-format".to_string()),
            "cargo test should not get --message-format injected");
    }

    // ── toolchain override (+nightly) tests ───────────────────────────────────

    #[test]
    fn toolchain_override_build_injected() {
        let handler = CargoHandler;
        let args: Vec<String> = vec!["cargo", "+nightly", "build"]
            .into_iter().map(String::from).collect();
        let result = handler.rewrite_args(&args);
        assert!(result.contains(&"--message-format".to_string()),
            "cargo +nightly build should get --message-format injected: {:?}", result);
        assert!(result.contains(&"+nightly".to_string()),
            "toolchain token should be preserved: {:?}", result);
    }

    #[test]
    fn toolchain_override_clippy_injected() {
        let handler = CargoHandler;
        let args: Vec<String> = vec!["cargo", "+stable", "clippy"]
            .into_iter().map(String::from).collect();
        let result = handler.rewrite_args(&args);
        assert!(result.contains(&"--message-format".to_string()),
            "cargo +stable clippy should get --message-format injected: {:?}", result);
    }

    #[test]
    fn toolchain_override_test_not_injected() {
        let handler = CargoHandler;
        let args: Vec<String> = vec!["cargo", "+nightly", "test"]
            .into_iter().map(String::from).collect();
        let result = handler.rewrite_args(&args);
        assert!(!result.contains(&"--message-format".to_string()),
            "cargo +nightly test should not get --message-format: {:?}", result);
    }

    #[test]
    fn cargo_subcmd_basic() {
        let args: Vec<String> = vec!["cargo".into(), "build".into()];
        assert_eq!(cargo_subcmd(&args), "build");
    }

    #[test]
    fn cargo_subcmd_with_toolchain() {
        let args: Vec<String> = vec!["cargo".into(), "+nightly".into(), "check".into()];
        assert_eq!(cargo_subcmd(&args), "check");
    }

    #[test]
    fn cargo_subcmd_empty() {
        let args: Vec<String> = vec!["cargo".into()];
        assert_eq!(cargo_subcmd(&args), "");
    }

    // ── error cap ────────────────────────────────────────────────────────────

    fn make_error_json(i: usize) -> String {
        format!(
            r#"{{"reason":"compiler-message","message":{{"level":"error","message":"error {}","spans":[]}}}}"#,
            i
        )
    }

    #[test]
    fn errors_capped_at_15_shows_overflow_line() {
        let lines: Vec<String> = (0..20).map(make_error_json).collect();
        let output = lines.join("\n");
        let result = filter_build(&output);
        let error_lines: Vec<&str> = result.lines().filter(|l| l.starts_with("error:")).collect();
        assert_eq!(error_lines.len(), 15, "should show exactly 15 errors");
        assert!(result.contains("[+5 more errors]"), "should show overflow line");
    }

    #[test]
    fn errors_under_cap_no_overflow_line() {
        let lines: Vec<String> = (0..10).map(make_error_json).collect();
        let output = lines.join("\n");
        let result = filter_build(&output);
        assert!(!result.contains("more errors"), "should not show overflow line");
    }

    // ── filter_nextest ───────────────────────────────────────────────────────

    #[test]
    fn nextest_fail_and_summary_kept_pass_dropped() {
        let output = "\
PASS [0.001s] mycrate::tests::passing_test
FAIL [0.002s] mycrate::tests::failing_test
Summary [0.003s] 2 tests run, 1 failed
";
        let result = filter_nextest(output);
        assert!(result.contains("FAIL"), "should keep FAIL line");
        assert!(result.contains("Summary"), "should keep Summary line");
        assert!(!result.contains("PASS"), "should drop PASS lines");
    }

    #[test]
    fn nextest_all_pass_returns_summary_only() {
        let output = "\
PASS [0.001s] mycrate::tests::test_a
PASS [0.001s] mycrate::tests::test_b
Summary [0.002s] 2 tests run, 0 failed
";
        let result = filter_nextest(output);
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 1, "should return only summary line");
        assert!(lines[0].starts_with("Summary"));
    }

    #[test]
    fn nextest_list_passthrough() {
        let handler = CargoHandler;
        let args: Vec<String> = vec!["cargo".into(), "nextest".into(), "list".into()];
        let output = "mycrate::tests::test_a\nmycrate::tests::test_b\n";
        let result = handler.filter(output, &args);
        assert_eq!(result, output, "cargo nextest list should pass through unchanged");
    }

    #[test]
    fn fewer_than_three_warnings_shown_as_is() {
        let warnings = vec![
            "warning: something [some_lint] [src/a.rs:1]".to_string(),
            "warning: something else [other_lint] [src/b.rs:2]".to_string(),
        ];
        let result = group_clippy_warnings(&warnings);
        assert_eq!(result.len(), 2);
        assert!(result[0].starts_with("  "));
        assert!(result[1].starts_with("  "));
    }

    // ── filter_test (compact summary) ───────────────────────────────────────

    #[test]
    fn test_all_pass_compact_summary() {
        let output = "\
running 42 tests
test foo::bar ... ok
test foo::baz ... ok
test result: ok. 42 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out; finished in 1.23s
";
        let result = filter_test(output);
        assert_eq!(result, "cargo test: 42 passed, 3 ignored (1 suite, 1.23s)");
    }

    #[test]
    fn test_all_pass_multi_suite_aggregation() {
        let output = "\
running 10 tests
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.50s

running 20 tests
test result: ok. 20 passed; 0 failed; 2 ignored; 0 measured; 5 filtered out; finished in 1.00s
";
        let result = filter_test(output);
        assert!(result.starts_with("cargo test: 30 passed"), "got: {}", result);
        assert!(result.contains("2 ignored"), "got: {}", result);
        assert!(result.contains("5 filtered"), "got: {}", result);
        assert!(result.contains("2 suites"), "got: {}", result);
    }

    #[test]
    fn test_failures_capped_at_10() {
        let mut lines = Vec::new();
        lines.push("running 15 tests".to_string());
        for i in 0..15 {
            lines.push(format!("test fail_{} ... FAILED", i));
        }
        lines.push("test result: FAILED. 0 passed; 15 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s".to_string());
        let output = lines.join("\n");
        let result = filter_test(&output);
        let failed_lines: Vec<&str> = result.lines().filter(|l| l.starts_with("FAILED:")).collect();
        assert_eq!(failed_lines.len(), 10, "should cap at 10 failures, got: {}", failed_lines.len());
        assert!(result.contains("[+5 more failures]"), "should show overflow, got: {}", result);
    }
}
