use super::Handler;

pub struct EmberHandler;

impl Handler for EmberHandler {
    fn filter(&self, output: &str, args: &[String]) -> String {
        let subcmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
        match subcmd {
            "build" | "b" => filter_build(output),
            "test" | "t" => filter_test(output),
            "serve" | "s" => filter_serve(output),
            // generate/destroy output is already short — passthrough
            _ => output.to_string(),
        }
    }
}

fn filter_build(output: &str) -> String {
    let mut errors: Vec<&str> = Vec::new();
    let mut summary: Option<&str> = None;

    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        // Final summary lines — covers "Build successful", "Built project successfully", "Build failed"
        if (t.starts_with("Build") || t.starts_with("Built"))
            && (t.contains("successful") || t.contains("failed"))
        {
            summary = Some(line);
            continue;
        }
        // Error lines: TypeScript errors, template errors, JS/TS file references
        if t.contains("Error:")
            || t.contains("error TS")
            || t.contains(".hbs:")
            || t.contains(".js:")
            || t.contains(".ts:")
            || t.contains(".gjs:")
            || t.contains(".gts:")
        {
            errors.push(line);
        }
        // Everything else (progress lines, fingerprint spam) is dropped
    }

    // Cap at 40 error lines
    let capped: Vec<&str> = errors.iter().copied().take(40).collect();
    let extra = errors.len().saturating_sub(40);

    let mut out: Vec<String> = capped.iter().map(|l| l.to_string()).collect();
    if extra > 0 {
        out.push(format!("[+{} more errors]", extra));
    }
    if let Some(s) = summary {
        out.push(s.to_string());
    }

    if out.is_empty() {
        // No errors and no summary — output was likely just progress noise
        output.to_string()
    } else {
        out.join("\n")
    }
}

fn filter_test(output: &str) -> String {
    // Parse counters from TAP summary lines: "# tests N", "# pass N", "# fail N"
    let mut total_count: Option<u32> = None;
    let mut pass_count: Option<u32> = None;
    let mut fail_count: Option<u32> = None;

    // Also detect the plan line "1..N" for total
    let lines: Vec<&str> = output.lines().collect();

    for line in &lines {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("1..") {
            if let Ok(n) = rest.trim().parse::<u32>() {
                total_count = Some(n);
            }
            continue;
        }
        if t.starts_with("# ") {
            let inner = t[2..].trim();
            if let Some(n_str) = inner.strip_prefix("tests ") {
                if let Ok(n) = n_str.trim().parse::<u32>() {
                    total_count = Some(n);
                }
            } else if let Some(n_str) = inner.strip_prefix("pass ") {
                if let Ok(n) = n_str.trim().parse::<u32>() {
                    pass_count = Some(n);
                }
            } else if let Some(n_str) = inner.strip_prefix("fail ") {
                if let Ok(n) = n_str.trim().parse::<u32>() {
                    fail_count = Some(n);
                }
            }
        }
    }

    // Collect failing test blocks (not ok lines + YAML diagnostic blocks)
    const MAX_FAILURES: usize = 5;
    const MAX_DIAG_LINES: usize = 8;

    let mut failure_blocks: Vec<String> = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let t = lines[i].trim();
        if t.starts_with("not ok") {
            let mut block: Vec<String> = vec![lines[i].to_string()];
            i += 1;
            let mut diag_lines = 0;
            // Consume diagnostic block: lines that are indented or YAML markers
            while i < lines.len() && diag_lines < MAX_DIAG_LINES {
                let next = lines[i];
                let nt = next.trim();
                // Stop at next test result line or plan line
                if nt.starts_with("ok ") || nt.starts_with("not ok") || nt.starts_with("1..") {
                    break;
                }
                // Stop at empty line (end of YAML block)
                if nt.is_empty() {
                    i += 1;
                    break;
                }
                // Skip bare YAML delimiters but still consume them
                if nt == "---" || nt == "..." {
                    i += 1;
                    diag_lines += 1;
                    continue;
                }
                // Skip TAP summary comment lines inside a block
                if nt.starts_with("# tests ") || nt.starts_with("# pass ") || nt.starts_with("# fail ") {
                    break;
                }
                block.push(next.to_string());
                diag_lines += 1;
                i += 1;
            }
            failure_blocks.push(block.join("\n"));
        } else {
            i += 1;
        }
    }

    // Build output
    if failure_blocks.is_empty() {
        // All passed — emit compact summary
        return match (pass_count, total_count) {
            (Some(p), _) => format!("Ember Tests: {} passed", p),
            (None, Some(t)) => format!("Ember Tests: {} passed", t),
            _ => "[all tests passed]".to_string(),
        };
    }

    let total_failures = failure_blocks.len();
    let shown: Vec<String> = failure_blocks.into_iter().take(MAX_FAILURES).collect();
    let mut out = shown;
    if total_failures > MAX_FAILURES {
        out.push(format!("[+{} more failures]", total_failures - MAX_FAILURES));
    }

    // Summary line
    let summary = match (pass_count, fail_count, total_count) {
        (Some(p), Some(f), Some(t)) => format!("Ember Tests: {}/{} passed, {} failed", p, t, f),
        (Some(p), Some(f), None) => format!("Ember Tests: {} passed, {} failed", p, f),
        (Some(p), None, Some(t)) => format!("Ember Tests: {}/{} passed", p, t),
        _ => String::new(),
    };
    if !summary.is_empty() {
        out.push(summary);
    }

    out.join("\n")
}

fn filter_serve(output: &str) -> String {
    // Keep only the "Serving on http://localhost:XXXX" line
    for line in output.lines() {
        let t = line.trim();
        if t.starts_with("Serving on") || t.contains("http://localhost") {
            return line.to_string();
        }
    }
    // Fallback: passthrough if no serving line found
    output.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::Handler;

    fn args(subcmd: &str) -> Vec<String> {
        vec!["ember".to_string(), subcmd.to_string()]
    }

    // ── filter_build ──────────────────────────────────────────────────────────

    #[test]
    fn build_keeps_error_lines_and_summary() {
        let output = "Building...\n\
                      app/templates/index.hbs:5:3: Error: Unexpected token\n\
                      app/components/foo.ts:12:1: error TS2345: Argument of type 'string'\n\
                      Built project successfully (1234ms)";
        let result = EmberHandler.filter(output, &args("build"));
        assert!(result.contains("Error: Unexpected token"), "should keep .hbs error");
        assert!(result.contains("error TS2345"), "should keep TS error");
        assert!(!result.contains("Building..."), "should drop progress noise");
        assert!(result.contains("Built project successfully"), "should keep summary");
    }

    #[test]
    fn build_failed_summary_kept() {
        let output = "Building...\nsome noise\nBuild failed.";
        let result = EmberHandler.filter(output, &args("build"));
        assert!(result.contains("Build failed"));
    }

    #[test]
    fn build_caps_at_40_errors() {
        let many_errors: String = (0..50)
            .map(|i| format!("app/components/x.js:{}:1: Error: msg {}\n", i, i))
            .collect();
        let result = EmberHandler.filter(&many_errors, &args("build"));
        assert!(
            result.contains("[+10 more errors]"),
            "should cap at 40 and show overflow: got {:?}",
            result
        );
    }

    #[test]
    fn build_passthrough_when_no_errors_or_summary() {
        let output = "Building...\nSome random non-error progress line";
        let result = EmberHandler.filter(output, &args("build"));
        assert_eq!(result, output);
    }

    // ── filter_test ───────────────────────────────────────────────────────────

    #[test]
    fn test_all_pass_tap_format() {
        let output = "TAP version 13\n\
                      1..60\n\
                      ok 1 - auth/login: renders login form\n\
                      ok 2 - auth/login: validates required fields\n\
                      ok 3 - auth/register: renders registration form\n\
                      # tests 3\n\
                      # pass 3\n\
                      # fail 0";
        let result = EmberHandler.filter(output, &args("test"));
        assert!(
            result.contains("passed"),
            "should indicate all passed: got {:?}",
            result
        );
        assert!(
            !result.contains("not ok"),
            "should not contain any failures: got {:?}",
            result
        );
    }

    #[test]
    fn test_all_pass_no_counts_returns_sentinel() {
        let output = "ok 1 - test\nok 2 - test2";
        let result = EmberHandler.filter(output, &args("test"));
        assert_eq!(
            result, "[all tests passed]",
            "should return sentinel when no counts: got {:?}",
            result
        );
    }

    #[test]
    fn test_failing_with_yaml_diagnostic_block() {
        let output = "\
TAP version 13
1..5
ok 1 - auth/login: renders login form
ok 2 - auth/login: validates required fields
not ok 3 - auth/register: sends confirmation email after signup
  ---
  message: Expected test to resolve to true, got false
  severity: failed
  at:
    line: 23
    column: 5
  ...
ok 4 - auth/login: redirects on success
ok 5 - auth/login: shows error on bad password
# tests 5
# pass 4
# fail 1";
        let result = EmberHandler.filter(output, &args("test"));
        assert!(result.contains("not ok 3"), "should keep failing test line");
        assert!(
            result.contains("Expected test to resolve to true"),
            "should include YAML diagnostic message: got {:?}",
            result
        );
        assert!(
            !result.contains("ok 1 - auth"),
            "should drop passing test lines"
        );
        assert!(
            result.contains("4/5 passed") || result.contains("4 passed"),
            "should include pass/fail counts: got {:?}",
            result
        );
        assert!(result.contains("1 failed"), "should include fail count");
    }

    #[test]
    fn test_tap_summary_counts_extracted() {
        let output = "\
TAP version 13
1..60
ok 1 - foo
ok 2 - bar
not ok 3 - baz fails
  ---
  message: boom
  ...
# tests 60
# pass 57
# fail 3";
        let result = EmberHandler.filter(output, &args("test"));
        assert!(
            result.contains("57/60 passed") || result.contains("57 passed"),
            "should show pass count: got {:?}",
            result
        );
        assert!(result.contains("3 failed"), "should show fail count: got {:?}", result);
    }

    #[test]
    fn test_more_than_5_failures_capped() {
        let mut output = String::from("TAP version 13\n1..10\n");
        for i in 1..=10 {
            output.push_str(&format!("not ok {} - test {} fails\n", i, i));
        }
        output.push_str("# tests 10\n# pass 0\n# fail 10\n");

        let result = EmberHandler.filter(&output, &args("test"));
        // Count the "not ok" lines in output — should be exactly 5
        let not_ok_count = result.lines().filter(|l| l.trim().starts_with("not ok")).count();
        assert_eq!(not_ok_count, 5, "should cap at 5 failures: got {:?}", result);
        assert!(
            result.contains("[+5 more failures]"),
            "should show overflow marker: got {:?}",
            result
        );
    }

    #[test]
    fn test_drops_passing_keeps_failing_summary() {
        let output = "ok 1 - MyApp: foo passes\n\
                      not ok 2 - MyApp: bar fails\n\
                      # tests 2\n\
                      # pass 1\n\
                      # fail 1";
        let result = EmberHandler.filter(output, &args("test"));
        assert!(result.contains("not ok 2"), "should keep failing test");
        assert!(!result.contains("ok 1 - MyApp: foo"), "should drop passing test");
        assert!(
            result.contains("1 failed"),
            "should include fail count: got {:?}",
            result
        );
    }

    // ── filter_serve ──────────────────────────────────────────────────────────

    #[test]
    fn serve_keeps_only_localhost_line() {
        let output = "Build successful (1234ms)\n\
                      Serving on http://localhost:4200\n\
                      Watching for changes...";
        let result = EmberHandler.filter(output, &args("serve"));
        assert_eq!(result.trim(), "Serving on http://localhost:4200");
    }

    #[test]
    fn serve_passthrough_when_no_serving_line() {
        let output = "Starting server...\nInitializing...";
        let result = EmberHandler.filter(output, &args("serve"));
        assert_eq!(result, output);
    }

    // ── generate / destroy passthrough ────────────────────────────────────────

    #[test]
    fn generate_passthrough() {
        let output = "installing component\n  create app/components/my-widget.js\n  create tests/integration/components/my-widget-test.js";
        let result = EmberHandler.filter(output, &args("generate"));
        assert_eq!(result, output);
    }

    #[test]
    fn destroy_passthrough() {
        let output = "removing component\n  remove app/components/my-widget.js";
        let result = EmberHandler.filter(output, &args("destroy"));
        assert_eq!(result, output);
    }

    #[test]
    fn short_alias_b_routes_to_build() {
        let output = "Building...\nBuild failed.";
        let result = EmberHandler.filter(output, &args("b"));
        assert!(result.contains("Build failed"));
    }

    #[test]
    fn short_alias_s_routes_to_serve() {
        let output = "Build successful\nServing on http://localhost:4200\nWatching...";
        let result = EmberHandler.filter(output, &args("s"));
        assert_eq!(result.trim(), "Serving on http://localhost:4200");
    }
}
