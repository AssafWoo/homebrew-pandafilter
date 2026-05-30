use super::Handler;

/// Dedicated handler for pnpm — covers install, add, run, test, exec.
/// pnpm output differs from npm: progress lines, lockfile messages, workspace info.
pub struct PnpmHandler;

impl Handler for PnpmHandler {
    fn filter(&self, output: &str, args: &[String]) -> String {
        let subcmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
        match subcmd {
            "install" | "i" | "add" | "update" | "up" | "remove" | "rm" => filter_install(output),
            "run" | "exec" => filter_run(output),
            "test" | "t" => filter_test(output),
            "dlx" => filter_run(output),
            _ => output.to_string(),
        }
    }
}

fn filter_install(output: &str) -> String {
    let mut packages_summary: Option<String> = None;
    let mut progress_summary: Option<String> = None;
    let mut warnings: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        // "Packages: +42 -3" summary
        if t.starts_with("Packages:") {
            packages_summary = Some(t.to_string());
            continue;
        }
        // "Progress: resolved N, reused N, downloaded N, added N, done"
        if t.starts_with("Progress:") && t.contains("done") {
            progress_summary = Some(t.to_string());
            continue;
        }
        // Warnings like "WARN  deprecated ..."
        if t.starts_with("WARN") || t.contains(" deprecated ") {
            if warnings.len() < 5 {
                warnings.push(t.trim_start_matches("WARN").trim().to_string());
            }
            continue;
        }
        // Errors
        if t.starts_with("ERR") || t.starts_with("error") {
            errors.push(t.to_string());
            continue;
        }
        // Drop: progress bars (+++), lock file messages, resolution lines
    }

    if !errors.is_empty() {
        return errors.join("\n");
    }

    let mut out: Vec<String> = Vec::new();
    if let Some(pkg) = packages_summary {
        out.push(pkg);
    }
    if let Some(prog) = progress_summary {
        out.push(prog);
    } else {
        out.push("[install complete]".to_string());
    }
    for w in &warnings {
        out.push(format!("warn: {}", w));
    }
    if out.is_empty() {
        output.to_string()
    } else {
        out.join("\n")
    }
}

fn filter_run(output: &str) -> String {
    // Detect test runner JSON output (vitest --reporter=json or jest --json)
    if let Some(compact) = super::util::parse_js_test_json(output) {
        return compact;
    }
    // Detect vitest verbose text output ("Test Files N passed (N)")
    if output.contains("Test Files") && (output.contains(" passed") || output.contains(" failed")) {
        return super::util::test_failures(output, "vitest");
    }
    // Detect jest text output ("Test Suites:" + "Tests:")
    if output.contains("Test Suites:") && output.contains("Tests:") {
        return super::util::test_failures(output, "jest");
    }

    let lines: Vec<&str> = output.lines().collect();
    if lines.len() <= 20 {
        return output.to_string();
    }

    let mut important: Vec<String> = lines
        .iter()
        .filter(|l| {
            let lower = l.to_lowercase();
            lower.contains("error")
                || lower.contains("warn")
                || lower.contains("failed")
                || lower.contains("success")
                || lower.contains("done in")
                || lower.contains("built in")
                || lower.contains("compiled")
        })
        .map(|l| l.to_string())
        .collect();

    // Always include last 5 lines
    let tail: Vec<String> = lines[lines.len().saturating_sub(5)..]
        .iter()
        .map(|l| l.to_string())
        .collect();

    important.push(format!("[{} lines of output]", lines.len()));
    important.extend(tail);
    important.dedup();
    important.join("\n")
}

fn filter_test(output: &str) -> String {
    // Delegate to the same logic as npm test filter
    let mut failures: Vec<String> = Vec::new();
    let mut summary_lines: Vec<String> = Vec::new();
    let mut in_failure = false;

    for line in output.lines() {
        let t = line.trim();
        if t.starts_with("✕") || t.starts_with("✗") || t.starts_with("× ") || t.contains("FAIL ") {
            failures.push(t.to_string());
        }
        if t.contains("failing") || t.contains("passed") || t.contains("failed") {
            summary_lines.push(t.to_string());
        }
        if t.starts_with('●') {
            in_failure = true;
        }
        if in_failure {
            failures.push(t.to_string());
            if t.is_empty() {
                in_failure = false;
            }
        }
    }

    if failures.is_empty() && !summary_lines.is_empty() {
        return summary_lines.join("\n");
    }

    let mut out: Vec<String> = failures;
    if let Some(last) = summary_lines.last() {
        out.push(last.clone());
    }
    if out.is_empty() {
        output.to_string()
    } else {
        out.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::Handler;

    fn args(subcmd: &str) -> Vec<String> {
        vec!["pnpm".to_string(), subcmd.to_string()]
    }

    #[test]
    fn install_extracts_packages_summary() {
        let output = "\
Packages: +5 -2
+++++--
Progress: resolved 540, reused 535, downloaded 5, added 5, done
";
        let result = PnpmHandler.filter(output, &args("install"));
        assert!(result.contains("Packages: +5 -2"));
        assert!(result.contains("Progress:"));
        assert!(!result.contains("+++++--"));
    }

    #[test]
    fn install_no_summary_falls_back() {
        let output = "Already up to date.\n";
        let result = PnpmHandler.filter(output, &args("install"));
        assert!(!result.is_empty());
    }

    #[test]
    fn install_shows_warnings() {
        let output = "\
Packages: +1\nProgress: resolved 10, reused 9, downloaded 1, added 1, done\n\
WARN  deprecated old-package@1.0.0: use new-package instead\n";
        let result = PnpmHandler.filter(output, &args("add"));
        assert!(result.contains("deprecated") || result.contains("old-package"));
    }

    #[test]
    fn run_short_output_passthrough() {
        let output = "done in 0.3s\n";
        let result = PnpmHandler.filter(output, &args("run"));
        assert_eq!(result, output);
    }

    #[test]
    fn run_long_output_compressed() {
        let line = "some output line\n";
        let output = line.repeat(50);
        let result = PnpmHandler.filter(&output, &args("run"));
        assert!(result.contains("lines of output"));
    }
}
