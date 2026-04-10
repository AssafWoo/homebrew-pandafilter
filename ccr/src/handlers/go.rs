use super::util;
use super::Handler;

pub struct GoHandler;

impl Handler for GoHandler {
    fn rewrite_args(&self, args: &[String]) -> Vec<String> {
        let subcmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
        if subcmd == "test" && !args.iter().any(|a| a == "-json") {
            let mut out = args.to_vec();
            // Insert -json after "test"
            out.insert(2, "-json".to_string());
            return out;
        }
        args.to_vec()
    }

    fn filter(&self, output: &str, args: &[String]) -> String {
        let subcmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
        // go tool golangci-lint → delegate to golangci-lint handler
        if subcmd == "tool" && args.get(2).map(|s| s.as_str()) == Some("golangci-lint") {
            return crate::handlers::golangci_lint::filter_lint(output);
        }
        match subcmd {
            "build" | "install" | "vet" => filter_build(output),
            "test" => filter_test(output),
            "run" => filter_run(output),
            "mod" => filter_mod(output),
            _ => output.to_string(),
        }
    }
}

fn filter_build(output: &str) -> String {
    // Go build errors look like: "path/file.go:42:5: error message"
    let errors: Vec<&str> = output
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty()
                && (t.contains(": undefined")
                    || t.contains(": cannot")
                    || t.contains(": syntax error")
                    || t.contains(": declared and not used")
                    || t.contains(": imported and not used")
                    || t.contains(": too many")
                    || t.contains(": not enough")
                    || t.contains(": ambiguous")
                    || t.contains(": multiple")
                    || (t.contains(".go:") && t.contains(": ")))
        })
        .collect();

    if errors.is_empty() {
        if output.trim().is_empty() {
            return "[build OK]".to_string();
        }
        return output.to_string();
    }
    errors.join("\n")
}

/// Strip module prefix from a package path, returning just the last segment.
/// e.g. `github.com/user/repo/pkg/server` → `server`
fn compact_package_name(pkg: &str) -> &str {
    pkg.rsplit('/').next().unwrap_or(pkg)
}

/// Parse `go test -json` structured output.
/// Tracks per-package pass/fail/skip counts, groups failures under their package,
/// and emits compact per-package summary lines.
fn filter_test_json(output: &str) -> String {
    use std::collections::HashMap;

    // Per-test buffered output lines
    let mut test_output: HashMap<String, Vec<String>> = HashMap::new();
    // Per-package counts: (pass, fail, skip)
    let mut pkg_counts: HashMap<String, (usize, usize, usize)> = HashMap::new();

    let mut failures: Vec<String> = Vec::new();
    let mut pkg_summaries: Vec<String> = Vec::new();
    let mut any_failure = false;

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Non-JSON lines (e.g. panic output) — keep verbatim
        if !line.starts_with('{') {
            failures.push(line.to_string());
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let action = v.get("Action").and_then(|a| a.as_str()).unwrap_or("");
        let test   = v.get("Test").and_then(|t| t.as_str()).unwrap_or("").to_string();
        let pkg    = v.get("Package").and_then(|p| p.as_str()).unwrap_or("?").to_string();

        let counts = pkg_counts.entry(pkg.clone()).or_insert((0, 0, 0));

        match action {
            "output" => {
                if let Some(out) = v.get("Output").and_then(|o| o.as_str()) {
                    test_output.entry(test).or_default().push(out.to_string());
                }
            }
            "pass" if !test.is_empty() => {
                counts.0 += 1;
                test_output.remove(&test); // discard passing test output
            }
            "skip" if !test.is_empty() => {
                counts.2 += 1;
                test_output.remove(&test);
            }
            "fail" if !test.is_empty() => {
                counts.1 += 1;
                any_failure = true;
                failures.push(format!("--- FAIL: {}", test));
                if let Some(lines) = test_output.remove(&test) {
                    let mut count = 0usize;
                    for l in &lines {
                        let l = l.trim_end_matches('\n');
                        let t = l.trim();
                        if t.is_empty()
                            || t.starts_with("=== RUN")
                            || t.starts_with("--- FAIL")
                            || t.starts_with("--- PASS")
                        {
                            continue;
                        }
                        failures.push(format!("    {}", l));
                        count += 1;
                        if count >= 10 {
                            failures.push("    [... truncated ...]".to_string());
                            break;
                        }
                    }
                }
            }
            "fail" if test.is_empty() => {
                // Package-level failure — emit compact summary
                any_failure = true;
                let (p, f, s) = pkg_counts.get(&pkg).copied().unwrap_or((0, 0, 0));
                let short = compact_package_name(&pkg);
                if p > 0 || f > 0 || s > 0 {
                    let mut parts = vec![format!("{} passed", p)];
                    if f > 0 { parts.push(format!("{} failed", f)); }
                    if s > 0 { parts.push(format!("{} skipped", s)); }
                    pkg_summaries.push(format!("FAIL {} [{}]", short, parts.join(", ")));
                } else {
                    let elapsed = v.get("Elapsed").and_then(|e| e.as_f64()).unwrap_or(0.0);
                    pkg_summaries.push(format!("FAIL\t{}\t{:.3}s", pkg, elapsed));
                }
            }
            "pass" if test.is_empty() => {
                // Package-level pass — emit compact summary
                let (p, _, s) = pkg_counts.get(&pkg).copied().unwrap_or((0, 0, 0));
                let short = compact_package_name(&pkg);
                if p > 0 || s > 0 {
                    let mut parts = vec![format!("{} passed", p)];
                    if s > 0 { parts.push(format!("{} skipped", s)); }
                    pkg_summaries.push(format!("ok  {} [{}]", short, parts.join(", ")));
                } else {
                    let elapsed = v.get("Elapsed").and_then(|e| e.as_f64()).unwrap_or(0.0);
                    pkg_summaries.push(format!("ok  \t{}\t{:.3}s", pkg, elapsed));
                }
            }
            _ => {}
        }
    }

    let total_pass: usize = pkg_counts.values().map(|(p, _, _)| *p).sum();

    let mut out: Vec<String> = failures;
    out.extend(pkg_summaries);

    if !any_failure && out.iter().all(|l| l.starts_with("ok  ")) {
        if total_pass > 0 {
            out.push(format!("[{} tests passed]", total_pass));
        } else {
            return "[all tests passed]".to_string();
        }
        return out.join("\n");
    }

    if total_pass > 0 {
        out.push(format!("[{} tests passed]", total_pass));
    }

    if out.is_empty() {
        "[all tests passed]".to_string()
    } else {
        out.join("\n")
    }
}

fn filter_test(output: &str) -> String {
    // Detect `-json` mode: first non-empty line starts with '{'
    if output.lines().find(|l| !l.trim().is_empty()).map(|l| l.trim_start().starts_with('{')).unwrap_or(false) {
        return filter_test_json(output);
    }
    let lines: Vec<&str> = output.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut in_failure = false;
    let mut failure_lines = 0usize;
    let mut pass_count = 0usize;
    let mut all_pass = true; // true until we see a FAIL marker

    for line in &lines {
        let t = line.trim();

        // Explicitly drop framework noise
        if t.starts_with("=== RUN")
            || t.starts_with("=== PAUSE")
            || t.starts_with("=== CONT")
            || t.starts_with("coverage:")
        {
            continue;
        }

        // "PASS" alone on a line — note it, emit summary later
        if t == "PASS" {
            // will be handled by the final summary; just skip the raw line
            continue;
        }

        // Count passing individual tests
        if t.starts_with("--- PASS:") {
            pass_count += 1;
            // Do not emit these lines
            if in_failure {
                // A PASS marker ends an active failure block
                in_failure = false;
            }
            continue;
        }

        // FAIL markers
        if t.starts_with("FAIL") || t.starts_with("--- FAIL:") {
            all_pass = false;
            out.push(line.to_string());
            in_failure = true;
            failure_lines = 0;
            continue;
        }

        // Panic lines
        if t.starts_with("panic:") || t.starts_with("goroutine ") {
            all_pass = false;
            out.push(line.to_string());
            continue;
        }

        if in_failure {
            if failure_lines < 10 {
                out.push(line.to_string());
                failure_lines += 1;
            } else if failure_lines == 10 {
                out.push("[... truncated ...]".to_string());
                failure_lines += 1;
            }
            // Blank line ends failure block (after we have a few lines of context)
            if t.is_empty() && failure_lines > 2 {
                in_failure = false;
            }
            continue;
        }

        // Summary: ok / FAIL with package + time
        if (t.starts_with("ok ") || t.starts_with("FAIL\t") || t.starts_with("FAIL "))
            && t.contains('\t')
        {
            out.push(line.to_string());
            continue;
        }

        // Error / hard-keep output
        if util::is_hard_keep(t) {
            out.push(line.to_string());
        }
    }

    // Append pass summary
    if all_pass && out.is_empty() {
        return "[all tests passed]".to_string();
    }
    if pass_count > 0 {
        out.push(format!("[{} tests passed]", pass_count));
    }

    if out.is_empty() {
        output.to_string()
    } else {
        out.join("\n")
    }
}

fn filter_run(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() <= 50 {
        return output.to_string();
    }
    // Traceback / panic: keep from the panic line onward
    if let Some(pos) = output.find("goroutine 1 [running]:") {
        return output[pos..].to_string();
    }
    if let Some(pos) = output.find("panic:") {
        return output[pos..].to_string();
    }
    // Long output: BERT summarize
    let result = panda_core::summarizer::summarize(output, 40);
    result.output
}

fn filter_mod(output: &str) -> String {
    // go mod tidy / download — keep warnings and errors only
    let important: Vec<&str> = output
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty()
                && (util::is_hard_keep(t)
                    || t.starts_with("go: ")
                    || t.contains("module")
                    || t.contains("version"))
        })
        .take(20)
        .collect();
    if important.is_empty() {
        "[go mod complete]".to_string()
    } else {
        important.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── filter_build ──────────────────────────────────────────────────────────

    #[test]
    fn build_ok_for_empty_output() {
        assert_eq!(filter_build(""), "[build OK]");
        assert_eq!(filter_build("   \n  "), "[build OK]");
    }

    #[test]
    fn build_extracts_go_error_lines() {
        let output = "# mypackage\n\
                      ./main.go:10:5: undefined: Foo\n\
                      ./main.go:20:3: cannot use x (type int) as type string\n\
                      some unrelated output line";
        let result = filter_build(output);
        assert!(result.contains("undefined: Foo"), "should keep undefined error");
        assert!(result.contains("cannot use x"), "should keep cannot error");
        assert!(!result.contains("unrelated"), "should drop noise");
    }

    #[test]
    fn build_keeps_ambiguous_and_multiple_errors() {
        let output = "# pkg\n./a.go:5:1: ambiguous import: found package foo\n./b.go:9:2: multiple-value in single-value context";
        let result = filter_build(output);
        assert!(result.contains("ambiguous import"), "should keep ambiguous");
        assert!(result.contains("multiple-value"), "should keep multiple");
    }

    #[test]
    fn build_passthrough_when_no_errors_but_has_output() {
        let output = "some non-error output that does not match patterns";
        let result = filter_build(output);
        assert_eq!(result, output);
    }

    // ── filter_test ───────────────────────────────────────────────────────────

    #[test]
    fn test_strips_run_and_pass_lines_keeps_fail() {
        let output = "=== RUN   TestFoo\n\
                      --- PASS: TestFoo (0.00s)\n\
                      === RUN   TestBar\n\
                      --- FAIL: TestBar (0.01s)\n\
                          bar_test.go:42: expected 1 got 2\n\
                      FAIL\tmy/pkg\t0.123s";
        let result = filter_test(output);
        assert!(!result.contains("=== RUN"), "should strip RUN markers");
        assert!(!result.contains("--- PASS:"), "should strip PASS lines");
        assert!(result.contains("--- FAIL: TestBar"), "should keep FAIL line");
        assert!(result.contains("expected 1 got 2"), "should keep failure detail");
        assert!(result.contains("FAIL\tmy/pkg"), "should keep package summary");
    }

    #[test]
    fn test_emits_pass_count_summary() {
        let output = "=== RUN   TestAlpha\n\
                      --- PASS: TestAlpha (0.00s)\n\
                      === RUN   TestBeta\n\
                      --- PASS: TestBeta (0.00s)\n\
                      === RUN   TestGamma\n\
                      --- PASS: TestGamma (0.00s)\n\
                      PASS\n\
                      ok  \tmy/pkg\t0.010s";
        let result = filter_test(output);
        assert!(
            result.contains("[3 tests passed]"),
            "should emit pass count: got {:?}",
            result
        );
    }

    #[test]
    fn test_pure_pass_output_emits_all_tests_passed() {
        let output = "=== RUN   TestOne\n\
                      --- PASS: TestOne (0.00s)\n\
                      PASS";
        let result = filter_test(output);
        // With one passing test and no failures the summary should fire
        assert!(
            result.contains("[all tests passed]") || result.contains("[1 tests passed]"),
            "should indicate all passed: got {:?}",
            result
        );
    }

    #[test]
    fn test_strips_coverage_lines() {
        let output = "=== RUN   TestFoo\n\
                      --- PASS: TestFoo (0.00s)\n\
                      coverage: 82.5% of statements\n\
                      ok  \tmy/pkg\t0.050s";
        let result = filter_test(output);
        assert!(!result.contains("coverage:"), "should strip coverage lines");
    }

    #[test]
    fn test_strips_pause_and_cont_markers() {
        let output = "=== RUN   TestParallel\n\
                      === PAUSE TestParallel\n\
                      === CONT  TestParallel\n\
                      --- PASS: TestParallel (0.00s)\n\
                      PASS";
        let result = filter_test(output);
        assert!(!result.contains("=== PAUSE"), "should strip PAUSE");
        assert!(!result.contains("=== CONT"), "should strip CONT");
    }

    #[test]
    fn test_failure_detail_truncated_after_10_lines() {
        let detail: String = (0..20).map(|i| format!("detail line {}\n", i)).collect();
        let output = format!(
            "--- FAIL: TestBig (0.00s)\n{}\nFAIL\tpkg\t0.1s",
            detail
        );
        let result = filter_test(&output);
        assert!(
            result.contains("[... truncated ...]"),
            "should truncate long failure blocks"
        );
    }

    // ── filter_mod ────────────────────────────────────────────────────────────

    #[test]
    fn mod_complete_for_empty_output() {
        assert_eq!(filter_mod(""), "[go mod complete]");
    }

    #[test]
    fn mod_keeps_go_prefix_lines() {
        let output = "go: downloading github.com/foo/bar v1.2.3\nsome noise\n";
        let result = filter_mod(output);
        assert!(result.contains("go: downloading"), "should keep go: lines");
        assert!(!result.contains("some noise"), "should drop noise");
    }

    // ── compact_package_name ──────────────────────────────────────────────────

    #[test]
    fn compact_strips_module_prefix() {
        assert_eq!(compact_package_name("github.com/user/repo/pkg/server"), "server");
        assert_eq!(compact_package_name("github.com/user/repo"), "repo");
    }

    #[test]
    fn compact_leaves_short_name_unchanged() {
        assert_eq!(compact_package_name("server"), "server");
        assert_eq!(compact_package_name(""), "");
    }

    // ── filter_test_json improvements ─────────────────────────────────────────

    fn json_event(action: &str, pkg: &str, test: Option<&str>, output: Option<&str>) -> String {
        let test_field = test
            .map(|t| format!(r#","Test":"{}""#, t))
            .unwrap_or_default();
        let out_field = output
            .map(|o| format!(r#","Output":"{}""#, o.replace('\n', "\\n")))
            .unwrap_or_default();
        format!(
            r#"{{"Action":"{}","Package":"{}"{}{}}}"#,
            action, pkg, test_field, out_field
        )
    }

    #[test]
    fn json_test_compact_package_name_in_pass_summary() {
        let pkg = "github.com/user/repo/pkg/util";
        let lines = vec![
            json_event("run",  pkg, Some("TestU1"), None),
            json_event("pass", pkg, Some("TestU1"), None),
            json_event("run",  pkg, Some("TestU2"), None),
            json_event("pass", pkg, Some("TestU2"), None),
            json_event("pass", pkg, None, None),
        ];
        let result = filter_test_json(&lines.join("\n"));
        // The summary should use the short name "util", not the full module path
        assert!(
            result.contains("util") && !result.contains("github.com"),
            "should compact package name: got {:?}",
            result
        );
    }

    #[test]
    fn json_test_compact_package_name_in_fail_summary() {
        let pkg = "github.com/user/repo/pkg/server";
        // One failing test, then package fail
        let lines = vec![
            json_event("run", pkg, Some("TestFoo"), None),
            json_event("output", pkg, Some("TestFoo"), Some("    foo_test.go:5: expected 1 got 2\n")),
            json_event("fail", pkg, Some("TestFoo"), None),
            json_event("fail", pkg, None, None),
        ];
        let result = filter_test_json(&lines.join("\n"));
        assert!(
            result.contains("server") && !result.contains("github.com"),
            "should compact package name in FAIL summary: got {:?}",
            result
        );
    }

    #[test]
    fn json_test_per_package_counts_in_summary() {
        let pkg = "github.com/user/repo/pkg/api";
        let lines = vec![
            json_event("run",  pkg, Some("TestA"), None),
            json_event("pass", pkg, Some("TestA"), None),
            json_event("run",  pkg, Some("TestB"), None),
            json_event("pass", pkg, Some("TestB"), None),
            json_event("run",  pkg, Some("TestC"), None),
            json_event("output", pkg, Some("TestC"), Some("    c_test.go:1: boom\n")),
            json_event("fail", pkg, Some("TestC"), None),
            json_event("fail", pkg, None, None),
        ];
        let result = filter_test_json(&lines.join("\n"));
        // Summary should say "2 passed, 1 failed"
        assert!(
            result.contains("2 passed") && result.contains("1 failed"),
            "should include per-package counts: got {:?}",
            result
        );
    }

    #[test]
    fn json_test_multi_package_mixed() {
        let pkg_ok  = "github.com/user/repo/pkg/util";
        let pkg_bad = "github.com/user/repo/pkg/api";
        let lines = vec![
            // util: 3 passing
            json_event("run",  pkg_ok,  Some("TestU1"), None),
            json_event("pass", pkg_ok,  Some("TestU1"), None),
            json_event("run",  pkg_ok,  Some("TestU2"), None),
            json_event("pass", pkg_ok,  Some("TestU2"), None),
            json_event("run",  pkg_ok,  Some("TestU3"), None),
            json_event("pass", pkg_ok,  Some("TestU3"), None),
            json_event("pass", pkg_ok,  None, None),
            // api: 1 passing, 1 failing
            json_event("run",  pkg_bad, Some("TestA1"), None),
            json_event("pass", pkg_bad, Some("TestA1"), None),
            json_event("run",  pkg_bad, Some("TestA2"), None),
            json_event("output", pkg_bad, Some("TestA2"), Some("    api_test.go:9: want 0 got 1\n")),
            json_event("fail", pkg_bad, Some("TestA2"), None),
            json_event("fail", pkg_bad, None, None),
        ];
        let result = filter_test_json(&lines.join("\n"));
        assert!(result.contains("ok  util"), "should show ok util summary");
        assert!(result.contains("FAIL api"), "should show FAIL api summary");
        assert!(result.contains("1 passed, 1 failed"), "api should show counts");
        assert!(result.contains("3 passed"), "util should show 3 passed");
    }

    // ── go tool golangci-lint routing ─────────────────────────────────────────

    #[test]
    fn tool_golangci_lint_routes_to_lint_handler() {
        let args: Vec<String> = vec![
            "go".to_string(),
            "tool".to_string(),
            "golangci-lint".to_string(),
            "run".to_string(),
        ];
        let output = "src/main.go:10:5: unused variable x (deadcode)\n\
                      INFO [runner] done";
        let result = GoHandler.filter(output, &args);
        // Should NOT pass through raw; should apply golangci-lint filtering
        assert!(!result.contains("INFO [runner]"), "should drop INFO lines");
        assert!(result.contains("main.go"), "should keep diagnostic");
    }

    #[test]
    fn tool_golangci_lint_clean_run() {
        let args: Vec<String> = vec![
            "go".to_string(),
            "tool".to_string(),
            "golangci-lint".to_string(),
            "run".to_string(),
        ];
        let output = "INFO [config] Config search paths: [/home/user]\nINFO [loader] done\n";
        let result = GoHandler.filter(output, &args);
        assert!(result.contains("No issues") || result == "No issues found.");
    }
}
