/// Pure regex pre-filter — runs unconditionally on all outputs before any BERT processing.
/// Goal: strip lines that are always noise regardless of context, with zero BERT cost.
use once_cell::sync::Lazy;
use regex::Regex;

// Progress bar lines — two forms:
//   Bracketed: [=======>     ] or [####  56%]  (bracket required; spaces inside ok)
//   Bare long: ======== (8+ repeating bar chars, no surrounding text)
static RE_PROGRESS_BAR: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?x)
        ^\s*
        (?:
            [\[(] \s* [=\-\#\*>▓░▒]{3,} [\s=\-\#\*>▓░▒]* [\])]   # bracketed bar
            |
            [=\-\#\*>]{8,}                                           # bare long bar ≥8 chars
        )
        \s* \d{0,3} \.? \d* \s* %? \s*
        $",
    )
    .unwrap()
});

// Download / transfer progress: "Downloading 45.2 MB", "Fetching 12%", "Receiving objects: 34%"
static RE_DOWNLOAD: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)^\s*(downloading|fetching|pulling|pushing|receiving objects|resolving deltas|writing objects|counting objects|compressing objects|remote:)\s",
    )
    .unwrap()
});

// Spinner-only lines (Unicode Braille spinners + ASCII /-\|)
static RE_SPINNER: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\s*[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏|/\\\-]\s*$").unwrap()
});

// "X% done", "34/100 files", standalone percentage line
static RE_PERCENT_LINE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\s*\d{1,3}(\.\d+)?\s*%\s*(done|complete|completed|of \d+)?\s*$").unwrap()
});

// Bare "X/Y" ratio lines (e.g. "12/50")
static RE_RATIO_LINE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\s*\d+\s*/\s*\d+\s*$").unwrap()
});

// Cargo/rustc build progress lines that are never informative:
//   "   Compiling foo v0.1.0 (/path)"
//   "   Checking foo v0.1.0 (/path)"
//   "    Fresh foo v0.1.0"
// Keep: Finished, Installing, Installed, error, warning — those are meaningful.
static RE_BUILD_PROGRESS: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\s{2,}(Compiling|Checking|Fresh)\s+\S").unwrap()
});

/// Returns true if a line consists entirely of separator/decorator characters
/// and is long enough (≥10 chars) to be structural rather than meaningful.
/// Conservative: requires 100% of non-space chars to be from the separator set.
fn is_pure_decorator(s: &str) -> bool {
    let non_space: Vec<char> = s.chars().filter(|c| !c.is_whitespace()).collect();
    if non_space.len() < 10 {
        return false; // short lines like "---" can be YAML/diff separators — keep them
    }
    non_space
        .iter()
        .all(|c| matches!(c, '─' | '═' | '━' | '-' | '=' | '*' | '#' | '~' | '+' | '_' | '▓' | '░' | '▒' | '·' | '•'))
}

/// Apply global pre-filter rules to `input`.
/// Strips: progress bars, download progress lines, spinners, pure decorator lines.
/// Preserves all other lines unchanged.
pub fn apply(input: &str) -> String {
    let lines: Vec<&str> = input.lines().collect();
    let mut out: Vec<&str> = Vec::with_capacity(lines.len());

    for line in &lines {
        let t = line.trim();

        // Empty lines pass through (blank-line collapsing is whitespace.rs's job)
        if t.is_empty() {
            out.push(line);
            continue;
        }

        // Progress bars: [=====>  ] style
        if RE_PROGRESS_BAR.is_match(t) {
            continue;
        }

        // Download/transfer progress lines
        if RE_DOWNLOAD.is_match(t) {
            continue;
        }

        // Spinner-only lines
        if RE_SPINNER.is_match(t) {
            continue;
        }

        // Standalone percentage / ratio lines
        if RE_PERCENT_LINE.is_match(t) || RE_RATIO_LINE.is_match(t) {
            continue;
        }

        // Pure long decorator lines (e.g. 40-char lines of ─────────)
        if is_pure_decorator(t) {
            continue;
        }

        // Cargo/rustc build progress (Compiling/Checking/Fresh — never informative)
        if RE_BUILD_PROGRESS.is_match(t) {
            continue;
        }

        out.push(line);
    }

    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_progress_bar() {
        let input = "Starting build\n[=======>     ]\n[========>    ] 80%\nDone";
        let result = apply(input);
        assert!(result.contains("Starting build"), "should keep regular lines");
        assert!(result.contains("Done"), "should keep regular lines");
        assert!(!result.contains("[=======>"), "progress bar should be stripped");
        assert!(!result.contains("80%"), "percentage bar should be stripped");
    }

    #[test]
    fn strips_download_lines() {
        let input = "Preparing\nDownloading 45.2 MB of 120 MB\nFetching index\nComplete";
        let result = apply(input);
        assert!(result.contains("Preparing"));
        assert!(result.contains("Complete"));
        assert!(!result.contains("Downloading"), "download line should be stripped");
        assert!(!result.contains("Fetching"), "fetch line should be stripped");
    }

    #[test]
    fn strips_unicode_spinner() {
        let input = "Running tests\n⠙\n⠹\nAll done";
        let result = apply(input);
        assert!(result.contains("Running tests"));
        assert!(result.contains("All done"));
        assert!(!result.contains('⠙'));
        assert!(!result.contains('⠹'));
    }

    #[test]
    fn strips_ascii_spinner_line() {
        let input = "Loading\n/\n-\n\\\n|\nDone";
        let result = apply(input);
        assert!(result.contains("Loading"));
        assert!(result.contains("Done"));
        assert!(!result.contains("\n/\n"), "ascii spinner should be stripped");
    }

    #[test]
    fn strips_long_decorator_line() {
        let input = "Header\n──────────────────────────────────────\nContent\n══════════════════════════════════════\nFooter";
        let result = apply(input);
        assert!(result.contains("Header"));
        assert!(result.contains("Content"));
        assert!(result.contains("Footer"));
        assert!(!result.contains("──────"), "long decorator should be stripped");
        assert!(!result.contains("══════"), "long separator should be stripped");
    }

    #[test]
    fn keeps_short_separator_lines() {
        // "---" (3 chars) is YAML/diff separator — must not be stripped
        let input = "key: value\n---\nnext: doc";
        let result = apply(input);
        assert!(result.contains("---"), "short separators must be preserved");
    }

    #[test]
    fn strips_standalone_percent_line() {
        let input = "Building\n  34%\n100% done\nFinished";
        let result = apply(input);
        assert!(result.contains("Building"));
        assert!(result.contains("Finished"));
        assert!(!result.contains("34%"));
        assert!(!result.contains("100% done"));
    }

    #[test]
    fn strips_cargo_build_progress() {
        let input = "   Compiling serde v1.0.0\n   Checking myapp v0.1.0\n    Fresh regex v1.5.0\nerror[E001]: bad\nwarning: unused\n   Finished dev [unoptimized] target(s) in 2.5s";
        let result = apply(input);
        assert!(!result.contains("Compiling"), "Compiling should be stripped");
        assert!(!result.contains("Checking"), "Checking should be stripped");
        assert!(!result.contains("Fresh"), "Fresh should be stripped");
        assert!(result.contains("error[E001]"), "errors must pass through");
        assert!(result.contains("warning"), "warnings must pass through");
        assert!(result.contains("Finished"), "Finished must pass through");
    }

    #[test]
    fn keeps_error_and_info_lines() {
        let input = "error: something went wrong\nwarning: check config\ninfo: starting";
        let result = apply(input);
        assert_eq!(result, input, "error/warning/info lines must pass through");
    }

    #[test]
    fn empty_input_returns_empty() {
        assert_eq!(apply(""), "");
    }

    #[test]
    fn passthrough_normal_output() {
        let input = "ok  github.com/foo/bar  (cached)\nFAIL github.com/foo/baz";
        let result = apply(input);
        assert_eq!(result, input);
    }

    #[test]
    fn is_pure_decorator_requires_min_length() {
        assert!(!is_pure_decorator("---"));     // too short
        assert!(!is_pure_decorator("====="));   // too short (< 10)
        assert!(is_pure_decorator("──────────────────────")); // long enough
        assert!(is_pure_decorator("──────────")); // exactly 10
    }

    #[test]
    fn is_pure_decorator_rejects_mixed_content() {
        assert!(!is_pure_decorator("-- some text --"));  // has letters
        assert!(!is_pure_decorator("=== RUN TestFoo")); // has letters
    }
}
