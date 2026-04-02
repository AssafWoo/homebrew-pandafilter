use once_cell::sync::OnceCell;
use regex::Regex;
use std::collections::BTreeMap;
use crate::handlers::Handler;

static RE_MYPY_ERROR: OnceCell<Regex> = OnceCell::new();
static RE_MYPY_SUMMARY: OnceCell<Regex> = OnceCell::new();
static RE_MYPY_DAEMON: OnceCell<Regex> = OnceCell::new();

fn re_mypy_error() -> &'static Regex {
    RE_MYPY_ERROR.get_or_init(|| {
        Regex::new(r"^(.+\.pyi?):(\d+): (error|note|warning): (.+?)(?:\s+\[[\w-]+\])?$").unwrap()
    })
}
fn re_mypy_summary() -> &'static Regex {
    RE_MYPY_SUMMARY.get_or_init(|| {
        Regex::new(r"^Found \d+ error|^Success: no issues found|^error: ").unwrap()
    })
}
fn re_mypy_daemon() -> &'static Regex {
    RE_MYPY_DAEMON.get_or_init(|| {
        Regex::new(r"(?i)^Daemon (started|stopped|already running)").unwrap()
    })
}

const MAX_ERRORS_PER_FILE: usize = 10;

pub struct MypyHandler;

impl Handler for MypyHandler {
    fn rewrite_args(&self, args: &[String]) -> Vec<String> {
        let mut out = args.to_vec();
        // Inject --no-color so regex patterns don't need to handle ANSI escapes
        if !out.iter().any(|a| a == "--no-color" || a == "--no-colour") {
            out.push("--no-color".to_string());
        }
        out
    }

    fn filter(&self, output: &str, _args: &[String]) -> String {
        filter_mypy(output)
    }
}

fn filter_mypy(output: &str) -> String {
    // Clean run
    if output.lines().any(|l| l.trim() == "Success: no issues found") {
        return "mypy: ok".to_string();
    }

    let mut file_errors: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut summary_lines: Vec<String> = Vec::new();

    for line in output.lines() {
        // Strip daemon lines
        if re_mypy_daemon().is_match(line) {
            continue;
        }
        // Keep summary lines verbatim
        if re_mypy_summary().is_match(line) {
            summary_lines.push(line.to_string());
            continue;
        }
        if let Some(caps) = re_mypy_error().captures(line) {
            let file = caps[1].to_string();
            let lineno = &caps[2];
            let level = &caps[3];
            let message = &caps[4];
            // Skip notes unless error level
            if level == "note" { continue; }
            let entry = format!("  line {}: {}", lineno, message);
            file_errors.entry(file).or_default().push(entry);
        } else if !line.trim().is_empty() && !re_mypy_daemon().is_match(line) {
            summary_lines.push(line.to_string());
        }
    }

    let mut out: Vec<String> = Vec::new();
    for (file, errors) in &file_errors {
        out.push(format!("{}:", file));
        let shown = errors.len().min(MAX_ERRORS_PER_FILE);
        for e in &errors[..shown] {
            out.push(e.clone());
        }
        if errors.len() > MAX_ERRORS_PER_FILE {
            out.push(format!("  [{} more errors in this file]", errors.len() - MAX_ERRORS_PER_FILE));
        }
    }
    out.extend(summary_lines);
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mypy_clean_returns_ok() {
        let handler = MypyHandler;
        let result = handler.filter("Success: no issues found\n", &[]);
        assert_eq!(result.trim(), "mypy: ok");
    }

    #[test]
    fn mypy_caps_errors_per_file() {
        let input: String = (1..=15)
            .map(|i| format!("src/app.py:{}: error: Cannot assign to a method [assignment]\n", i))
            .collect();
        let handler = MypyHandler;
        let result = handler.filter(&input, &[]);
        let more_line = result.lines().find(|l| l.contains("more errors"));
        assert!(more_line.is_some(), "no 'more errors' line for >10 errors");
        assert!(more_line.unwrap().contains("5"), "wrong count in 'more errors'");
    }

    #[test]
    fn mypy_summary_always_kept() {
        let mut input: String = (1..=3)
            .map(|i| format!("src/a.py:{}: error: Some error [misc]\n", i))
            .collect();
        input.push_str("Found 3 errors in 1 file (checked 5 source files)\n");
        let handler = MypyHandler;
        let result = handler.filter(&input, &[]);
        assert!(result.contains("Found 3 errors"), "summary line missing");
    }

    #[test]
    fn mypy_daemon_line_stripped() {
        let input = "Daemon started successfully\nsrc/a.py:1: error: Type error [misc]\nFound 1 error in 1 file\n";
        let handler = MypyHandler;
        let result = handler.filter(input, &[]);
        assert!(!result.contains("Daemon"), "daemon line not stripped");
    }

    #[test]
    fn mypy_multi_file_grouping() {
        let input = concat!(
            "src/a.py:1: error: Error in a [misc]\n",
            "src/b.py:1: error: Error in b [misc]\n",
            "src/a.py:2: error: Another error in a [misc]\n",
            "Found 3 errors in 2 files\n",
        );
        let handler = MypyHandler;
        let result = handler.filter(input, &[]);
        assert!(result.contains("src/a.py:"), "file a missing");
        assert!(result.contains("src/b.py:"), "file b missing");
    }
}
