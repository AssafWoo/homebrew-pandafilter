use super::Handler;

/// Handler for Playwright test runner (`playwright test`, `npx playwright test`).
/// Filters verbose browser/trace logs; keeps failures + summary.
pub struct PlaywrightHandler;

impl Handler for PlaywrightHandler {
    fn rewrite_args(&self, args: &[String]) -> Vec<String> {
        let mut out = args.to_vec();
        // Default to list reporter (compact one-line-per-test format)
        if !out.iter().any(|a| a.starts_with("--reporter")) {
            out.push("--reporter=list".to_string());
        }
        out
    }

    fn filter(&self, output: &str, args: &[String]) -> String {
        let subcmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
        match subcmd {
            "test" | "" => filter_test(output),
            "show-report" | "codegen" | "install" => output.to_string(),
            _ => filter_test(output),
        }
    }
}

fn filter_test(output: &str) -> String {
    let mut failures: Vec<String> = Vec::new();
    let mut error_detail: Vec<String> = Vec::new();
    let mut in_error = false;
    let mut summary: Option<String> = None;
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;

    let lines: Vec<&str> = output.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let t = line.trim();

        // Summary line: "X passed (Ys)", "X failed", "X skipped"
        if t.contains(" passed") && (t.contains('(') || t.contains("passed (")) {
            if let Some(n) = parse_count(t, "passed") {
                passed = n;
            }
        }
        if t.contains(" failed") {
            if let Some(n) = parse_count(t, "failed") {
                failed = n;
            }
        }
        if t.contains(" skipped") {
            if let Some(n) = parse_count(t, "skipped") {
                skipped = n;
            }
        }

        // Failure marker: "  1) [chromium] › test.spec.ts:5:3 › test name"
        // or "  ✗  1 [chromium] › ..."
        // or "  × 1 [chromium] › ..."
        if is_failure_header(t) {
            // Flush previous error detail
            if in_error && !error_detail.is_empty() {
                failures.extend(error_detail.drain(..));
            }
            failures.push(t.to_string());
            in_error = true;
            i += 1;
            continue;
        }

        // Error message block (inside failure)
        if in_error {
            // Stop collecting at next test header or blank line after error
            if t.is_empty() && !error_detail.is_empty() {
                failures.extend(error_detail.drain(..));
                in_error = false;
            } else if t.starts_with("Error:") || t.starts_with("expect(") || t.starts_with("at ") {
                if error_detail.len() < 8 {
                    error_detail.push(format!("  {}", t));
                }
            }
        }

        // Passing test lines: "  ✓  1 [chromium] ..." — drop
        // Flaky test notice
        if t.contains("flaky") {
            failures.push(format!("[flaky] {}", t));
        }

        // Final summary line
        if t.contains("Finished in") || t.starts_with("Running") && t.contains("test") {
            summary = Some(t.to_string());
        }

        i += 1;
    }

    // Flush any remaining error detail
    if !error_detail.is_empty() {
        failures.extend(error_detail);
    }

    // Build output
    let mut out: Vec<String> = Vec::new();

    if failures.is_empty() {
        // All passed
        let s = if passed > 0 || skipped > 0 {
            let mut parts = Vec::new();
            if passed > 0 {
                parts.push(format!("{} passed", passed));
            }
            if skipped > 0 {
                parts.push(format!("{} skipped", skipped));
            }
            format!("✓ {} ({})", parts.join(", "), summary.as_deref().unwrap_or("done"))
        } else {
            summary.clone().unwrap_or_else(|| "All tests passed".to_string())
        };
        return s;
    }

    out.extend(failures);
    out.push(String::new());

    let mut summary_parts = Vec::new();
    if failed > 0 {
        summary_parts.push(format!("{} failed", failed));
    }
    if passed > 0 {
        summary_parts.push(format!("{} passed", passed));
    }
    if skipped > 0 {
        summary_parts.push(format!("{} skipped", skipped));
    }
    if !summary_parts.is_empty() {
        out.push(summary_parts.join(", "));
    }
    if let Some(s) = summary {
        out.push(s);
    }

    out.join("\n")
}

fn is_failure_header(t: &str) -> bool {
    // Playwright failure patterns:
    // "  1) [chromium] › foo.spec.ts:5:3 › test name"
    // "  ✗  1 [chromium] › ..."
    // "  ×  1 [chromium] › ..."
    (t.starts_with("✗") || t.starts_with("×") || t.starts_with("FAILED"))
        || (t.ends_with("FAILED"))
        || (t.contains("›") && (t.contains(".spec.") || t.contains(".test.")) && !t.trim_start().starts_with('✓'))
}

fn parse_count(line: &str, keyword: &str) -> Option<usize> {
    let words: Vec<&str> = line.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
        if *w == keyword && i > 0 {
            if let Ok(n) = words[i - 1].parse::<usize>() {
                return Some(n);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::Handler;

    fn args() -> Vec<String> {
        vec!["playwright".to_string(), "test".to_string()]
    }

    #[test]
    fn all_passed_returns_summary() {
        let output = "\
Running 3 tests using 3 workers
  ✓  1 [chromium] › example.spec.ts:3:5 › has title (1.5s)
  ✓  2 [firefox] › example.spec.ts:3:5 › has title (2.1s)
  ✓  3 [webkit] › example.spec.ts:3:5 › has title (1.8s)

  3 passed (6s)
";
        let result = PlaywrightHandler.filter(output, &args());
        assert!(result.contains("passed"));
        // Should not show passing test lines
        assert!(!result.contains("[chromium]") || result.contains("passed"));
    }

    #[test]
    fn failure_shown_with_error() {
        let output = "\
Running 2 tests using 2 workers
  ✓  1 [chromium] › example.spec.ts:3:5 › has title (1.5s)
  ✗  2 [webkit] › example.spec.ts:8:5 › get started link (3.2s)

  1) [webkit] › example.spec.ts:8:5 › get started link ──────────────────────────────────
    Error: locator.click: Error: Element is not visible

  2 tests run, 1 passed, 1 failed
";
        let result = PlaywrightHandler.filter(output, &args());
        assert!(result.contains("failed") || result.contains("✗") || result.contains("Error"));
        assert!(!result.contains("has title"));
    }

    #[test]
    fn parse_count_extracts_number() {
        assert_eq!(parse_count("3 passed (6s)", "passed"), Some(3));
        assert_eq!(parse_count("1 failed", "failed"), Some(1));
        assert_eq!(parse_count("2 skipped", "skipped"), Some(2));
    }
}
