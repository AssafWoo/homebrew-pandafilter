use super::Handler;

/// Known npm built-in subcommands (not user scripts).
/// If the first argument is NOT in this list it is treated as a script name
/// and `run` is automatically injected before it.
const NPM_BUILTINS: &[&str] = &[
    "access", "adduser", "audit", "bin", "bugs", "cache", "ci", "completion",
    "config", "dedupe", "deprecate", "diff", "dist-tag", "docs", "doctor",
    "edit", "exec", "explain", "explore", "find-dupes", "fund", "get", "help",
    "help-search", "hook", "i", "init", "install", "install-ci-test",
    "install-test", "it", "link", "ll", "login", "logout", "ls", "ls",
    "org", "outdated", "owner", "pack", "ping", "pkg", "prefix", "profile",
    "prune", "publish", "query", "rebuild", "repo", "restart", "root",
    "run", "run-script", "search", "set", "set-script", "shrinkwrap", "star",
    "stars", "start", "stop", "t", "team", "test", "token", "tst", "un",
    "uninstall", "unlink", "unpublish", "unstar", "up", "update", "v",
    "version", "view", "whoami", "add", "remove", "rm", "r",
];

pub struct NpmHandler;

impl Handler for NpmHandler {
    fn rewrite_args(&self, args: &[String]) -> Vec<String> {
        let subcmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
        // Auto-inject `run` for user scripts (first arg not a known builtin)
        if !subcmd.is_empty() && !NPM_BUILTINS.contains(&subcmd) {
            let mut out = args.to_vec();
            out.insert(1, "run".to_string());
            return out;
        }
        match subcmd {
            "install" | "i" | "add" | "ci" => {
                if args.iter().any(|a| a == "--no-progress") {
                    args.to_vec()
                } else {
                    let mut out = args.to_vec();
                    out.push("--no-progress".to_string());
                    out
                }
            }
            _ => args.to_vec(),
        }
    }

    fn filter(&self, output: &str, args: &[String]) -> String {
        let subcmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
        match subcmd {
            "install" | "i" | "add" | "ci" => filter_install(output),
            "test" | "t" => filter_test(output),
            "run" | "run-script" => filter_run_script(output),
            "audit" => filter_audit(output),
            "outdated" => filter_outdated(output),
            _ => output.to_string(),
        }
    }
}

/// Returns true if a line is npm boilerplate that should be stripped.
fn is_boilerplate_line(line: &str) -> bool {
    let t = line.trim();

    // npm WARN or npm notice lines
    if t.starts_with("npm WARN") || t.starts_with("npm notice") {
        return true;
    }

    // Spinner/progress-only lines: only spaces, dots, /, -, \, |
    if !t.is_empty() && t.chars().all(|c| matches!(c, ' ' | '.' | '/' | '-' | '\\' | '|')) {
        return true;
    }

    // `> project@1.0.0 scriptname` lines (lifecycle script header)
    // Pattern: starts with `> `, then a package name (may contain @, ., /) followed by a space
    // and a script/command word.
    if is_lifecycle_header(t) {
        return true;
    }

    false
}

/// Detect lines like `> package@1.0.0 build` or `> @scope/pkg@2.3.1 start`.
fn is_lifecycle_header(t: &str) -> bool {
    if !t.starts_with("> ") {
        return false;
    }
    let rest = &t[2..];
    // Must have exactly one space separating "pkg@version" from "scriptname"
    // The package part contains at least one '@' or '.' or '/'
    let mut parts = rest.splitn(2, ' ');
    let pkg = parts.next().unwrap_or("");
    let script = parts.next().unwrap_or("").trim();
    if script.is_empty() {
        return false;
    }
    // Package part should look like a name (contains word chars, @, ., /)
    // Script part should be a single word (no spaces)
    let pkg_looks_valid = pkg.chars().any(|c| c == '@' || c == '.' || c == '/') || !pkg.is_empty();
    let script_is_word = !script.contains(' ') && script.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-');
    pkg_looks_valid && script_is_word
}

fn filter_install(output: &str) -> String {
    let mut package_count: Option<u32> = None;
    let mut audit_info: Option<String> = None;

    for line in output.lines() {
        let t = line.trim();

        // Skip boilerplate before examining content
        if is_boilerplate_line(line) {
            continue;
        }

        // npm: "added N packages"
        // pnpm: "N packages added"
        if let Some(n) = extract_package_count(t) {
            package_count = Some(n);
        }
        if t.contains("vulnerabilit") || t.contains("audit") {
            audit_info = Some(t.to_string());
        }
    }

    let count_str = package_count
        .map(|n| format!("{} packages", n))
        .unwrap_or_else(|| "packages".to_string());

    let mut out = format!("[install complete — {}]", count_str);
    if let Some(audit) = audit_info {
        out.push('\n');
        out.push_str(&audit);
    }
    out
}

fn extract_package_count(line: &str) -> Option<u32> {
    // "added 42 packages"
    let words: Vec<&str> = line.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
        if (*w == "added" || *w == "installed") && i + 1 < words.len() {
            if let Ok(n) = words[i + 1].parse::<u32>() {
                return Some(n);
            }
        }
    }
    None
}

fn filter_test(output: &str) -> String {
    // Parse test output — keep failures and final summary
    let mut failures: Vec<String> = Vec::new();
    let mut summary_lines: Vec<String> = Vec::new();
    let mut in_failure = false;

    for line in output.lines() {
        let t = line.trim();

        // Jest/vitest failure patterns
        if t.starts_with("✕") || t.starts_with("✗") || t.starts_with("× ") || t.contains("FAIL ") {
            failures.push(t.to_string());
        }

        // Mocha-style "N failing"
        if t.contains("failing") || t.contains("passed") || t.contains("failed") {
            summary_lines.push(t.to_string());
        }

        // Verbose failure output after "●"
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
    if !summary_lines.is_empty() {
        out.push(summary_lines.last().cloned().unwrap_or_default());
    }

    if out.is_empty() {
        output.to_string()
    } else {
        out.join("\n")
    }
}

fn filter_run_script(output: &str) -> String {
    // Strip boilerplate and empty lines before processing
    let cleaned: Vec<&str> = output
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !is_boilerplate_line(l)
        })
        .collect();

    // If output is short after stripping, pass through
    if cleaned.len() <= 30 {
        return cleaned.join("\n");
    }

    // If the output looks like linter output (majority of lines are diagnostics),
    // apply dedicated lint filtering instead of generic npm boilerplate stripping.
    let joined = cleaned.join("\n");
    if looks_like_lint_output(&joined) {
        return filter_lint_passthrough(&joined);
    }

    let mut important: Vec<String> = cleaned
        .iter()
        .filter(|l| {
            let lower = l.to_lowercase();
            lower.contains("error")
                || lower.contains("warning")
                || lower.contains("failed")
                || lower.contains("success")
                || lower.contains("done in")
                || lower.contains("built in")
        })
        .map(|l| l.to_string())
        .collect();

    // Always include last 5 lines of cleaned output
    let tail: Vec<String> = cleaned[cleaned.len().saturating_sub(5)..]
        .iter()
        .map(|l| l.to_string())
        .collect();

    important.push(format!("[{} lines of output]", cleaned.len()));
    important.extend(tail);
    important.dedup();
    important.join("\n")
}

/// Heuristic: ≥30% of non-blank lines look like linter diagnostics or file path headers.
///
/// Covers:
/// - Inline format: `path/file.ts:10:5: message`  (ruff, mypy, golangci-lint)
/// - Per-file-group: ESLint / TSC where file path is on its own line, then indented `10:5  error`
fn looks_like_lint_output(output: &str) -> bool {
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() < 5 {
        return false;
    }
    let lint_count = lines.iter()
        .filter(|l| is_diagnostic_line(l) || is_file_path_header(l.trim()))
        .count();
    lint_count * 100 / lines.len() >= 30
}

fn is_diagnostic_line(line: &str) -> bool {
    let t = line.trim();

    // ESLint per-file-group format: "  10:5   error   message"
    // These lines are indented; after trimming they start with digits:digits.
    {
        let without_digits = t.trim_start_matches(|c: char| c.is_ascii_digit());
        if without_digits.len() < t.len() && without_digits.starts_with(':') {
            let after_colon = &without_digits[1..];
            if after_colon.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                return true;
            }
        }
    }

    // path/to/file.ext:line:col or path/to/file.ext(line,col)
    // Match: non-whitespace chars, a dot, word chars, then colon+digits or paren+digits
    let bytes = t.as_bytes();
    let mut i = 0;
    let len = bytes.len();
    while i < len {
        if bytes[i] == b'.' {
            let ext_start = i + 1;
            let mut ext_end = ext_start;
            while ext_end < len && bytes[ext_end].is_ascii_alphabetic() {
                ext_end += 1;
            }
            let ext_len = ext_end - ext_start;
            if ext_len >= 1 && ext_len <= 6 && ext_end < len {
                let after = bytes[ext_end];
                if after == b':' || after == b'(' {
                    if ext_end + 1 < len && bytes[ext_end + 1].is_ascii_digit() {
                        return true;
                    }
                }
            }
        }
        i += 1;
    }
    false
}

/// True for lines that are standalone file path headers (ESLint per-file-group format).
/// Example: "/Users/dev/project/src/components/Button.tsx"
fn is_file_path_header(line: &str) -> bool {
    let t = line.trim();
    // Must contain a path separator (absolute or relative path)
    if !t.contains('/') && !t.contains('\\') {
        return false;
    }
    // Must end with a recognized file extension (2–6 alpha chars after last dot)
    if let Some(dot_pos) = t.rfind('.') {
        let ext = &t[dot_pos + 1..];
        if ext.len() >= 2 && ext.len() <= 6 && ext.chars().all(|c| c.is_ascii_alphanumeric()) {
            return true;
        }
    }
    false
}

/// Keep only error/warning/note diagnostic lines and the summary line.
/// Used when `npm run lint` wraps an actual linter.
fn filter_lint_passthrough(output: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut total = 0usize;
    let mut last_file_header: Option<&str> = None;

    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        total += 1;
        let lower = t.to_lowercase();

        // Track file path headers (ESLint per-file-group format).
        // Emit them lazily — only if a diagnostic follows in the same block.
        if is_file_path_header(t) {
            last_file_header = Some(line);
            continue;
        }

        let is_diag = is_diagnostic_line(t)
            || lower.contains("error")
            || lower.contains("warning")
            || lower.contains("problem")
            || lower.contains("✖")
            || lower.contains("✗")
            || lower.contains("✕");

        if is_diag {
            // Flush the pending file path header before the first diagnostic from this file.
            if let Some(header) = last_file_header.take() {
                kept.push(header);
            }
            kept.push(line);
        } else {
            // Non-diagnostic line resets the pending header (new section boundary).
            last_file_header = None;
        }
    }

    if kept.is_empty() {
        return output.to_string();
    }

    // Cap at 60 diagnostic lines; append summary of what was trimmed
    let cap = 60;
    let extra = kept.len().saturating_sub(cap);
    kept.truncate(cap);
    let mut result = kept.join("\n");
    if extra > 0 {
        result.push_str(&format!("\n[+{} more diagnostics]", extra));
    }
    result.push_str(&format!("\n[{} lines of linter output]", total));
    result
}

/// Filter `npm audit` output into a severity-grouped summary.
///
/// - No vulnerabilities → "[no vulnerabilities found]"
/// - Otherwise: extract the summary line + list affected packages with severity
fn filter_audit(output: &str) -> String {
    let t = output.trim();

    // Zero vulnerabilities: "found 0 vulnerabilities" or "0 vulnerabilities found"
    if t.contains("0 vulnerabilities") || t.contains("found 0 vulnerabilities") {
        return "[no vulnerabilities found]".to_string();
    }

    // Extract the footer summary line ("N vulnerabilities (X low, Y moderate, Z critical)")
    let summary = output.lines().rev().find(|l| {
        let t = l.trim();
        t.contains("vulnerabilit") && (t.contains("critical") || t.contains("high") || t.contains("moderate") || t.contains("low") || t.contains("found"))
    }).map(|l| l.trim().to_string());

    // Collect vulnerability entries: lines like "Severity: critical" preceded by a package name
    let lines: Vec<&str> = output.lines().collect();
    let mut packages: Vec<String> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.starts_with("Severity:") {
            let severity = t.trim_start_matches("Severity:").trim();
            // The package name is usually 1-2 lines above the Severity: line
            let pkg_line = lines[..i].iter().rev()
                .find(|l| {
                    let lt = l.trim();
                    !lt.is_empty()
                        && !lt.starts_with("npm")
                        && !lt.starts_with('#')
                        && !lt.starts_with("Depends on")
                        && !lt.starts_with("fix available")
                        && !lt.starts_with("node_modules")
                })
                .map(|l| l.trim())
                .unwrap_or("unknown");
            // Trim version range suffix (e.g. "lodash  <=4.17.20" → "lodash")
            let pkg_name = pkg_line.split_whitespace().next().unwrap_or(pkg_line);
            if packages.len() < 15 {
                packages.push(format!("{} ({})", pkg_name, severity));
            }
        }
    }

    let mut out = Vec::new();
    if let Some(s) = summary {
        out.push(s);
    }
    if !packages.is_empty() {
        out.push(packages.join(", "));
    }

    if out.is_empty() {
        // Couldn't parse — trim boilerplate and return a compact version
        let cleaned: Vec<&str> = output.lines()
            .filter(|l| {
                let t = l.trim();
                !t.is_empty() && !t.starts_with("npm audit report") && !t.starts_with("node_modules")
            })
            .take(20)
            .collect();
        return if cleaned.is_empty() { output.to_string() } else { cleaned.join("\n") };
    }
    out.join("\n")
}

/// Filter `npm outdated` output.
///
/// - Empty output → "[all packages up to date]"
/// - Otherwise: group into major vs minor upgrades, emit compact lines
fn filter_outdated(output: &str) -> String {
    if output.trim().is_empty() {
        return "[all packages up to date]".to_string();
    }

    let lines: Vec<&str> = output.lines().collect();
    // Find header line to determine column positions
    let header_idx = lines.iter().position(|l| {
        let t = l.trim();
        t.starts_with("Package") && t.contains("Current") && t.contains("Latest")
    });

    // Each entry: (package_name, current_version, latest_version)
    let mut entries: Vec<(String, String, String)> = Vec::new();

    if let Some(hi) = header_idx {
        let header = lines[hi];
        let current_col = header.find("Current").unwrap_or(20);
        let latest_col = header.find("Latest").unwrap_or(40);

        for line in &lines[hi + 1..] {
            if line.trim().is_empty() {
                continue;
            }
            let len = line.len();
            if len < 3 {
                continue;
            }
            let pkg = line[..current_col.min(len)].trim().to_string();
            let current = if current_col < len {
                let s = &line[current_col..latest_col.min(len)];
                s.split_whitespace().next().unwrap_or("").to_string()
            } else { continue };
            let latest = if latest_col < len {
                let s = &line[latest_col..];
                s.split_whitespace().next().unwrap_or("").to_string()
            } else { continue };

            if pkg.is_empty() || current.is_empty() || latest.is_empty() {
                continue;
            }
            if entries.len() < 40 {
                entries.push((pkg, current, latest));
            }
        }
    } else {
        // No recognized header — compact: keep non-empty lines, cap at 20
        let compact: Vec<&str> = lines.iter()
            .filter(|l| !l.trim().is_empty())
            .take(20)
            .copied()
            .collect();
        return format!("[{} packages outdated]\n{}", compact.len(), compact.join("\n"));
    }

    if entries.is_empty() {
        return "[all packages up to date]".to_string();
    }

    // Group by major version bump vs minor/patch
    let mut major: Vec<String> = Vec::new();
    let mut minor: Vec<String> = Vec::new();

    for (pkg, current, latest) in &entries {
        let cur_major = current.split('.').next().unwrap_or("0");
        let lat_major = latest.split('.').next().unwrap_or("0");
        let is_major = cur_major != lat_major;

        // Short display: pkg current→latest
        let display = format!("{} {}→{}", pkg, current, latest);
        if is_major {
            major.push(display);
        } else {
            minor.push(display);
        }
    }

    let mut out = vec![format!("[{} packages outdated]", entries.len())];
    if !major.is_empty() {
        out.push(format!("Major: {}", major.join(", ")));
    }
    if !minor.is_empty() {
        // Compact minor: just list package names with brief version bump
        let minor_compact: Vec<String> = minor.iter().take(15).cloned().collect();
        let extra = minor.len().saturating_sub(15);
        let mut s = format!("Minor: {}", minor_compact.join(", "));
        if extra > 0 {
            s.push_str(&format!(" [+{}]", extra));
        }
        out.push(s);
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::Handler;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn rewrite_args_injects_run_for_unknown_subcommand() {
        let handler = NpmHandler;
        // "build" is not a builtin — should become "npm run build"
        let result = handler.rewrite_args(&args(&["npm", "build"]));
        assert_eq!(result[1], "run", "should inject 'run'");
        assert_eq!(result[2], "build", "script name should remain");
    }

    #[test]
    fn rewrite_args_does_not_inject_run_for_builtins() {
        let handler = NpmHandler;
        let result = handler.rewrite_args(&args(&["npm", "install", "lodash"]));
        assert_eq!(result[1], "install", "'install' is a builtin — no run injection");
        let result2 = handler.rewrite_args(&args(&["npm", "test"]));
        assert_eq!(result2[1], "test", "'test' is a builtin — no run injection");
    }

    #[test]
    fn npm_warn_lines_dropped_from_install_output() {
        let handler = NpmHandler;
        let output = "\
npm WARN deprecated lodash@3.0.0: use lodash@4 instead
npm notice created a lockfile
added 42 packages from 30 contributors
npm WARN optional SKIPPING OPTIONAL DEPENDENCY";
        let result = handler.filter(output, &args(&["npm", "install"]));
        assert!(!result.contains("npm WARN"), "npm WARN lines should be stripped");
        assert!(!result.contains("npm notice"), "npm notice lines should be stripped");
        assert!(result.contains("42 packages"), "package count should be kept");
    }

    #[test]
    fn lifecycle_header_lines_dropped_from_install() {
        let handler = NpmHandler;
        let output = "\
> my-project@1.0.0 prepare
> husky install

added 10 packages";
        let result = handler.filter(output, &args(&["npm", "install"]));
        assert!(!result.contains("> my-project@1.0.0 prepare"), "lifecycle header should be stripped");
        assert!(result.contains("10 packages"), "package count should be kept");
    }

    #[test]
    fn package_count_summary_kept() {
        let handler = NpmHandler;
        let output = "\
npm WARN deprecated foo@1.0.0: bar
> project@0.1.0 postinstall
added 123 packages in 4.2s";
        let result = handler.filter(output, &args(&["npm", "install"]));
        assert!(result.contains("123 packages"), "package count summary must be present");
        assert!(!result.contains("npm WARN"), "WARN lines must be stripped");
    }

    #[test]
    fn run_script_strips_boilerplate_and_empty_lines() {
        // Build output with > lifecycle header and empty lines mixed in
        let mut lines: Vec<String> = vec![
            "> my-app@1.0.0 build".to_string(),
            String::new(),
        ];
        // Add 35 real output lines
        for i in 1..=35 {
            lines.push(format!("Building module {}", i));
        }
        lines.push("Build complete in 5s".to_string());
        let output = lines.join("\n");
        let result = filter_run_script(&output);
        // Should not contain the lifecycle header
        assert!(!result.contains("> my-app@1.0.0 build"), "lifecycle header should be stripped");
        // Should mention the success line
        assert!(result.contains("Build complete"), "important lines should be kept");
    }

    #[test]
    fn lint_output_detected_and_filtered() {
        // Simulate `npm run lint` wrapping eslint output
        let mut lines: Vec<String> = vec!["> my-app@1.0.0 lint".to_string()];
        for i in 1..=30 {
            lines.push(format!("src/components/Button.tsx:{}:5: error  no-unused-vars", i));
        }
        lines.push("✖ 30 problems (30 errors, 0 warnings)".to_string());
        let output = lines.join("\n");
        let result = filter_run_script(&output);
        // Should detect as lint and keep error lines + summary
        assert!(result.contains("Button.tsx"), "diagnostic lines should be kept");
        assert!(result.contains("✖"), "summary line should be kept");
        // Should contain total count annotation
        assert!(result.contains("linter output"), "lint annotation should be appended");
    }

    #[test]
    fn is_diagnostic_line_matches_common_patterns() {
        assert!(is_diagnostic_line("src/index.ts:10:5: error  no-unused-vars"));
        assert!(is_diagnostic_line("src/auth/jwt.rs:23:4: warning: unused variable"));
        assert!(is_diagnostic_line("lib/foo.js(42,7): error TS2304: Cannot find name"));
        assert!(!is_diagnostic_line("Building module 1"));
        assert!(!is_diagnostic_line("added 42 packages in 2.5s"));
    }

    // ── filter_audit ──────────────────────────────────────────────────────────

    #[test]
    fn audit_zero_vulnerabilities_returns_clean() {
        let output = "found 0 vulnerabilities";
        assert_eq!(filter_audit(output), "[no vulnerabilities found]");

        let output2 = "up to date, audited 512 packages\n0 vulnerabilities found";
        assert_eq!(filter_audit(output2), "[no vulnerabilities found]");
    }

    #[test]
    fn audit_extracts_summary_and_packages() {
        let output = "\
npm audit report

lodash  <=4.17.20
Severity: critical
Prototype Pollution - https://npmjs.com/advisories/1523
fix available via `npm audit fix`
node_modules/lodash

semver  6.0.0 - 6.3.0
Severity: high
ReDoS in semver - https://npmjs.com/advisories/1234
node_modules/semver

6 vulnerabilities (1 low, 2 moderate, 3 critical)";
        let result = filter_audit(output);
        assert!(result.contains("vulnerabilities"), "should contain summary, got: {}", result);
        assert!(result.contains("lodash"), "should list affected package, got: {}", result);
        assert!(result.contains("critical"), "should show severity, got: {}", result);
    }

    // ── filter_outdated ───────────────────────────────────────────────────────

    #[test]
    fn outdated_empty_returns_up_to_date() {
        assert_eq!(filter_outdated(""), "[all packages up to date]");
        assert_eq!(filter_outdated("   "), "[all packages up to date]");
    }

    #[test]
    fn outdated_parses_table_columns() {
        let output = "\
Package          Current   Wanted  Latest  Location
eslint           7.32.0    7.32.0  8.40.0  node_modules/eslint
react            17.0.2    17.0.2  18.2.0  my-app
typescript       4.9.5     4.9.5   5.3.3   my-app";
        let result = filter_outdated(output);
        assert!(result.contains("3 packages outdated"), "should count packages, got: {}", result);
        assert!(result.contains("eslint"), "should list eslint, got: {}", result);
        assert!(result.contains("8.40.0"), "should show latest version, got: {}", result);
        assert!(result.contains("react"), "should list react, got: {}", result);
    }
}
