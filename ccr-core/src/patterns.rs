use crate::config::{CommandConfig, FilterAction, MatchOutputConfig, SimpleAction};
use regex::Regex;

/// Build a collapse marker, optionally embedding a Zoom-In expand ID.
fn make_collapse_marker(count: usize, original_lines: Vec<String>) -> String {
    if crate::zoom::is_enabled() && !original_lines.is_empty() {
        let id = crate::zoom::register(original_lines);
        format!("[{} matching lines collapsed — ccr expand {}]", count, id)
    } else {
        format!("[{} matching lines collapsed]", count)
    }
}

struct CompiledPattern {
    regex: Regex,
    action: FilterAction,
    strip_ansi: bool,
}

pub struct PatternFilter {
    patterns: Vec<CompiledPattern>,
    /// Substitution returned when final output is blank.
    on_empty: Option<String>,
}

impl PatternFilter {
    pub fn new(config: &CommandConfig) -> anyhow::Result<Self> {
        let mut patterns = Vec::new();
        for p in &config.patterns {
            patterns.push(CompiledPattern {
                regex: Regex::new(&p.regex)?,
                action: p.action.clone(),
                strip_ansi: p.strip_ansi,
            });
        }
        Ok(Self {
            patterns,
            on_empty: config.on_empty.clone(),
        })
    }

    /// Returns true if `line` matches any Remove-action pattern for the current command.
    /// Used for streaming pre-filtering before BERT processing.
    pub fn should_remove(&self, line: &str) -> bool {
        for pat in &self.patterns {
            let test_line = if pat.strip_ansi {
                std::borrow::Cow::Owned(crate::ansi::strip_ansi(line))
            } else {
                std::borrow::Cow::Borrowed(line)
            };
            if pat.regex.is_match(&test_line) {
                if let FilterAction::Simple(SimpleAction::Remove) = &pat.action {
                    return true;
                }
            }
        }
        false
    }

    /// Apply all 8 filter stages to `input`:
    ///
    /// 1. Per-line: Remove / Collapse / ReplaceWith / TruncateLinesAt (with strip_ansi per rule)
    /// 2. Short-circuit: MatchOutput (checked against every line before line-level pass)
    /// 3. Output-level: HeadLines / TailLines
    /// 4. Final: OnEmpty
    pub fn apply(&self, input: &str) -> String {
        let lines: Vec<&str> = input.lines().collect();

        // ── Stage: MatchOutput short-circuit ─────────────────────────────────
        // Checked first: if any MatchOutput rule fires it bypasses all other stages.
        for pat in &self.patterns {
            let FilterAction::MatchOutput { MatchOutput: MatchOutputConfig { message, unless } } = &pat.action else {
                continue;
            };
            // Check if the pattern matches any line
            let matched = lines.iter().any(|line| {
                let test = if pat.strip_ansi {
                    std::borrow::Cow::Owned(crate::ansi::strip_ansi(line))
                } else {
                    std::borrow::Cow::Borrowed(*line)
                };
                pat.regex.is_match(&test)
            });

            if !matched {
                continue;
            }

            // Check unless suppressor
            if let Some(unless_pat) = unless {
                if let Ok(unless_re) = Regex::new(unless_pat) {
                    let suppressed = lines.iter().any(|line| unless_re.is_match(line));
                    if suppressed {
                        continue; // suppressed — don't short-circuit
                    }
                }
            }

            // Short-circuit: return message immediately
            return message.clone();
        }

        // ── Stage: line-level passes ──────────────────────────────────────────
        let mut result: Vec<String> = Vec::new();

        // Track consecutive matches per pattern index for Collapse.
        let mut collapse_counts: Vec<usize> = vec![0; self.patterns.len()];
        let mut collapsed_lines: Vec<Vec<String>> = vec![Vec::new(); self.patterns.len()];
        let mut active_collapse: Option<usize> = None;

        for line in &lines {
            let mut matched = false;
            for (i, pat) in self.patterns.iter().enumerate() {
                // Skip MatchOutput / HeadLines / TailLines / OnEmpty — handled elsewhere
                match &pat.action {
                    FilterAction::MatchOutput { .. }
                    | FilterAction::HeadLines { .. }
                    | FilterAction::TailLines { .. }
                    | FilterAction::OnEmpty { .. } => continue,
                    _ => {}
                }

                let test_line: std::borrow::Cow<'_, str> = if pat.strip_ansi {
                    std::borrow::Cow::Owned(crate::ansi::strip_ansi(line))
                } else {
                    std::borrow::Cow::Borrowed(line)
                };

                if !pat.regex.is_match(&test_line) {
                    continue;
                }

                matched = true;
                match &pat.action {
                    FilterAction::Simple(SimpleAction::Remove) => {
                        // Flush any active collapse from a different pattern
                        if let Some(ci) = active_collapse {
                            if ci != i {
                                if collapse_counts[ci] > 0 {
                                    let acc = std::mem::take(&mut collapsed_lines[ci]);
                                    result.push(make_collapse_marker(collapse_counts[ci], acc));
                                    collapse_counts[ci] = 0;
                                }
                                active_collapse = None;
                            }
                        }
                        // Remove — don't add to result
                    }
                    FilterAction::Simple(SimpleAction::Collapse) => {
                        // Flush different collapse
                        if let Some(ci) = active_collapse {
                            if ci != i {
                                if collapse_counts[ci] > 0 {
                                    let acc = std::mem::take(&mut collapsed_lines[ci]);
                                    result.push(make_collapse_marker(collapse_counts[ci], acc));
                                    collapse_counts[ci] = 0;
                                }
                            }
                        }
                        active_collapse = Some(i);
                        collapse_counts[i] += 1;
                        collapsed_lines[i].push(line.to_string());
                    }
                    FilterAction::ReplaceWith { ReplaceWith: replacement } => {
                        // Flush any active collapse
                        if let Some(ci) = active_collapse {
                            if collapse_counts[ci] > 0 {
                                let acc = std::mem::take(&mut collapsed_lines[ci]);
                                result.push(make_collapse_marker(collapse_counts[ci], acc));
                                collapse_counts[ci] = 0;
                            }
                            active_collapse = None;
                        }
                        result.push(replacement.clone());
                    }
                    FilterAction::TruncateLinesAt { TruncateLinesAt: max_chars } => {
                        // Flush any active collapse
                        if let Some(ci) = active_collapse {
                            if collapse_counts[ci] > 0 {
                                let acc = std::mem::take(&mut collapsed_lines[ci]);
                                result.push(make_collapse_marker(collapse_counts[ci], acc));
                                collapse_counts[ci] = 0;
                            }
                            active_collapse = None;
                        }
                        // Truncate the ORIGINAL line (not the ANSI-stripped version)
                        let chars: Vec<char> = line.chars().collect();
                        if chars.len() > *max_chars {
                            let truncated: String = chars[..*max_chars].iter().collect();
                            result.push(format!("{}…", truncated));
                        } else {
                            result.push(line.to_string());
                        }
                    }
                    // These are handled in other stages
                    FilterAction::MatchOutput { .. }
                    | FilterAction::HeadLines { .. }
                    | FilterAction::TailLines { .. }
                    | FilterAction::OnEmpty { .. } => {}
                }
                break;
            }

            if !matched {
                // Flush any active collapse
                if let Some(ci) = active_collapse {
                    if collapse_counts[ci] > 0 {
                        let acc = std::mem::take(&mut collapsed_lines[ci]);
                        result.push(make_collapse_marker(collapse_counts[ci], acc));
                        collapse_counts[ci] = 0;
                    }
                    active_collapse = None;
                }
                result.push(line.to_string());
            }
        }

        // Flush remaining collapse at end of input
        if let Some(ci) = active_collapse {
            if collapse_counts[ci] > 0 {
                let acc = std::mem::take(&mut collapsed_lines[ci]);
                result.push(make_collapse_marker(collapse_counts[ci], acc));
            }
        }

        // ── Stage: HeadLines / TailLines ──────────────────────────────────────
        // Applied in pattern order: last HeadLines / TailLines rule wins.
        for pat in &self.patterns {
            match &pat.action {
                FilterAction::HeadLines { HeadLines: n } => {
                    if result.len() > *n {
                        result.truncate(*n);
                    }
                }
                FilterAction::TailLines { TailLines: n } => {
                    if result.len() > *n {
                        let start = result.len() - n;
                        result = result[start..].to_vec();
                    }
                }
                _ => {}
            }
        }

        let output = result.join("\n");

        // ── Stage: OnEmpty ────────────────────────────────────────────────────
        // Command-level on_empty takes priority, then per-pattern OnEmpty.
        if output.trim().is_empty() {
            if let Some(msg) = &self.on_empty {
                return msg.clone();
            }
            for pat in &self.patterns {
                if let FilterAction::OnEmpty { OnEmpty: msg } = &pat.action {
                    return msg.clone();
                }
            }
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CommandConfig, FilterAction, FilterPattern, MatchOutputConfig, SimpleAction};

    fn make_config(patterns: Vec<FilterPattern>) -> CommandConfig {
        CommandConfig { patterns, on_empty: None }
    }

    #[test]
    fn removes_matching_regex_line() {
        let cfg = make_config(vec![FilterPattern {
            regex: "^noise.*".to_string(),
            action: FilterAction::Simple(SimpleAction::Remove),
            strip_ansi: false,
        }]);
        let filter = PatternFilter::new(&cfg).unwrap();
        let result = filter.apply("noise line\nkeep this");
        assert!(!result.contains("noise line"));
        assert!(result.contains("keep this"));
    }

    #[test]
    fn replaces_matching_line() {
        let cfg = make_config(vec![FilterPattern {
            regex: "^added \\d+ packages.*".to_string(),
            action: FilterAction::ReplaceWith {
                ReplaceWith: "[npm install complete]".to_string(),
            },
            strip_ansi: false,
        }]);
        let filter = PatternFilter::new(&cfg).unwrap();
        let result = filter.apply("added 42 packages in 5s");
        assert!(result.contains("[npm install complete]"));
    }

    #[test]
    fn collapse_repeated_pattern() {
        let cfg = make_config(vec![FilterPattern {
            regex: "^   Compiling \\S+ v[\\d.]+".to_string(),
            action: FilterAction::Simple(SimpleAction::Collapse),
            strip_ansi: false,
        }]);
        let filter = PatternFilter::new(&cfg).unwrap();
        let mut lines = Vec::new();
        for i in 0..50 {
            lines.push(format!("   Compiling crate{} v1.0", i));
        }
        let input = lines.join("\n");
        let result = filter.apply(&input);
        assert!(result.contains("50 matching lines collapsed"));
        let line_count = result.lines().count();
        assert_eq!(line_count, 1);
    }

    #[test]
    fn no_match_passthrough() {
        let cfg = make_config(vec![FilterPattern {
            regex: "^never_matches_xyz$".to_string(),
            action: FilterAction::Simple(SimpleAction::Remove),
            strip_ansi: false,
        }]);
        let filter = PatternFilter::new(&cfg).unwrap();
        let input = "line one\nline two\nline three";
        assert_eq!(filter.apply(input), input);
    }

    #[test]
    fn multiple_rules_applied_in_order() {
        let cfg = make_config(vec![
            FilterPattern {
                regex: "^noise.*".to_string(),
                action: FilterAction::Simple(SimpleAction::Remove),
                strip_ansi: false,
            },
            FilterPattern {
                regex: "^replace.*".to_string(),
                action: FilterAction::ReplaceWith {
                    ReplaceWith: "[replaced]".to_string(),
                },
                strip_ansi: false,
            },
        ]);
        let filter = PatternFilter::new(&cfg).unwrap();
        let input = "noise1\nreplace me\nkeep";
        let result = filter.apply(input);
        assert!(!result.contains("noise1"));
        assert!(result.contains("[replaced]"));
        assert!(result.contains("keep"));
    }

    // ── New DSL stage tests ───────────────────────────────────────────────────

    #[test]
    fn truncate_lines_at_truncates_long_lines() {
        let cfg = make_config(vec![FilterPattern {
            regex: ".*".to_string(),
            action: FilterAction::TruncateLinesAt { TruncateLinesAt: 10 },
            strip_ansi: false,
        }]);
        let filter = PatternFilter::new(&cfg).unwrap();
        let result = filter.apply("hello world this is a long line\nshort");
        let lines: Vec<&str> = result.lines().collect();
        assert!(lines[0].len() <= 14, "should be truncated"); // 10 chars + "…" (3 UTF-8 bytes)
        assert_eq!(lines[1], "short");
    }

    #[test]
    fn truncate_lines_at_leaves_short_lines_unchanged() {
        let cfg = make_config(vec![FilterPattern {
            regex: ".*".to_string(),
            action: FilterAction::TruncateLinesAt { TruncateLinesAt: 120 },
            strip_ansi: false,
        }]);
        let filter = PatternFilter::new(&cfg).unwrap();
        let input = "short line";
        assert_eq!(filter.apply(input), "short line");
    }

    #[test]
    fn head_lines_keeps_first_n() {
        let cfg = make_config(vec![FilterPattern {
            regex: ".*".to_string(),
            action: FilterAction::HeadLines { HeadLines: 3 },
            strip_ansi: false,
        }]);
        let filter = PatternFilter::new(&cfg).unwrap();
        let input = "a\nb\nc\nd\ne";
        let result = filter.apply(input);
        assert_eq!(result.lines().count(), 3);
        assert!(result.contains("a"));
        assert!(result.contains("c"));
        assert!(!result.contains("d"));
    }

    #[test]
    fn tail_lines_keeps_last_n() {
        let cfg = make_config(vec![FilterPattern {
            regex: ".*".to_string(),
            action: FilterAction::TailLines { TailLines: 2 },
            strip_ansi: false,
        }]);
        let filter = PatternFilter::new(&cfg).unwrap();
        let input = "a\nb\nc\nd\ne";
        let result = filter.apply(input);
        assert_eq!(result.lines().count(), 2);
        assert!(result.contains("d"));
        assert!(result.contains("e"));
        assert!(!result.contains("a"));
    }

    #[test]
    fn on_empty_fires_when_all_removed() {
        let mut cfg = make_config(vec![FilterPattern {
            regex: ".*".to_string(),
            action: FilterAction::Simple(SimpleAction::Remove),
            strip_ansi: false,
        }]);
        cfg.on_empty = Some("(nothing to do)".to_string());
        let filter = PatternFilter::new(&cfg).unwrap();
        let result = filter.apply("noise\nmore noise");
        assert_eq!(result, "(nothing to do)");
    }

    #[test]
    fn on_empty_not_fired_when_output_non_empty() {
        let mut cfg = make_config(vec![FilterPattern {
            regex: "^noise".to_string(),
            action: FilterAction::Simple(SimpleAction::Remove),
            strip_ansi: false,
        }]);
        cfg.on_empty = Some("empty!".to_string());
        let filter = PatternFilter::new(&cfg).unwrap();
        let result = filter.apply("noise line\nkeep this");
        assert!(result.contains("keep this"));
        assert!(!result.contains("empty!"));
    }

    #[test]
    fn match_output_short_circuits_on_match() {
        let cfg = make_config(vec![FilterPattern {
            regex: "error".to_string(),
            action: FilterAction::MatchOutput {
                MatchOutput: MatchOutputConfig {
                    message: "Build failed".to_string(),
                    unless: None,
                },
            },
            strip_ansi: false,
        }]);
        let filter = PatternFilter::new(&cfg).unwrap();
        let result = filter.apply("ok\nerror: something went wrong\nmore");
        assert_eq!(result, "Build failed");
    }

    #[test]
    fn match_output_not_triggered_when_no_match() {
        let cfg = make_config(vec![FilterPattern {
            regex: "error".to_string(),
            action: FilterAction::MatchOutput {
                MatchOutput: MatchOutputConfig {
                    message: "Build failed".to_string(),
                    unless: None,
                },
            },
            strip_ansi: false,
        }]);
        let filter = PatternFilter::new(&cfg).unwrap();
        let result = filter.apply("ok\nall good\ndone");
        assert!(result.contains("ok"));
        assert!(!result.contains("Build failed"));
    }

    #[test]
    fn match_output_suppressed_by_unless() {
        let cfg = make_config(vec![FilterPattern {
            regex: "error".to_string(),
            action: FilterAction::MatchOutput {
                MatchOutput: MatchOutputConfig {
                    message: "Build failed".to_string(),
                    unless: Some("warning_only".to_string()),
                },
            },
            strip_ansi: false,
        }]);
        let filter = PatternFilter::new(&cfg).unwrap();
        // Both "error" and "warning_only" match → unless suppresses the short-circuit
        let result = filter.apply("error: oops\nwarning_only: this is fine");
        assert!(!result.contains("Build failed"), "unless should suppress short-circuit");
    }

    #[test]
    fn strip_ansi_enables_matching_colored_line() {
        let cfg = make_config(vec![FilterPattern {
            regex: "^noise".to_string(),
            action: FilterAction::Simple(SimpleAction::Remove),
            strip_ansi: true,
        }]);
        let filter = PatternFilter::new(&cfg).unwrap();
        // Line with ANSI prefix around "noise" — without strip_ansi the regex would miss it
        let colored = "\x1b[31mnoise line\x1b[0m";
        let result = filter.apply(colored);
        assert!(!result.contains("noise"), "ANSI-colored noise should be removed");
    }
}
