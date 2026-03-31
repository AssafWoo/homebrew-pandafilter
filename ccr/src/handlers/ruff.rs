use once_cell::sync::OnceCell;
use regex::Regex;
use std::collections::BTreeMap;
use crate::handlers::Handler;

static RE_RUFF_VIOLATION: OnceCell<Regex> = OnceCell::new();
static RE_RUFF_SUMMARY: OnceCell<Regex> = OnceCell::new();

fn re_ruff_violation() -> &'static Regex {
    RE_RUFF_VIOLATION.get_or_init(|| {
        Regex::new(r"^(.+\.py):(\d+):(\d+): ([A-Z]\d+) (.+)$").unwrap()
    })
}
fn re_ruff_summary() -> &'static Regex {
    RE_RUFF_SUMMARY.get_or_init(|| {
        Regex::new(r"(?i)^Found \d+|^All checks passed").unwrap()
    })
}

pub struct RuffHandler;

impl Handler for RuffHandler {
    fn rewrite_args(&self, args: &[String]) -> Vec<String> {
        let subcmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
        if subcmd == "check" && !args.iter().any(|a| a.contains("--output-format")) {
            let mut out = args.to_vec();
            out.push("--output-format".to_string());
            out.push("concise".to_string());
            out
        } else {
            args.to_vec()
        }
    }

    fn filter(&self, output: &str, args: &[String]) -> String {
        let subcmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
        match subcmd {
            "check" => filter_ruff_check(output),
            "format" => filter_ruff_format(output),
            _ => output.to_string(),
        }
    }
}

fn filter_ruff_check(output: &str) -> String {
    // Check for clean run
    if output.lines().any(|l| l.trim() == "All checks passed.") {
        return "ruff: ok".to_string();
    }
    if output.trim().is_empty() {
        return "ruff: ok".to_string();
    }

    // Group violations by code
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut summary_lines: Vec<String> = Vec::new();
    let mut other_lines: Vec<String> = Vec::new();

    for line in output.lines() {
        if re_ruff_summary().is_match(line) {
            summary_lines.push(line.to_string());
            continue;
        }
        if let Some(caps) = re_ruff_violation().captures(line) {
            let code = caps[4].to_string();
            let location = format!("{}:{}:{}", &caps[1], &caps[2], &caps[3]);
            let msg = caps[5].to_string();
            groups.entry(code).or_default().push(format!("  {} — {}", location, msg));
        } else if !line.trim().is_empty() {
            other_lines.push(line.to_string());
        }
    }

    let mut out: Vec<String> = Vec::new();
    for (code, occurrences) in &groups {
        let count = occurrences.len();
        if count == 1 {
            out.push(format!("[{}]", code));
            out.push(occurrences[0].clone());
        } else {
            out.push(format!("[{} ×{}]", code, count));
            for o in occurrences.iter().take(3) {
                out.push(o.clone());
            }
            if count > 3 {
                out.push(format!("  ... and {} more", count - 3));
            }
        }
    }
    out.extend(other_lines);
    out.extend(summary_lines);
    out.join("\n")
}

fn filter_ruff_format(output: &str) -> String {
    let mut reformatted = 0usize;
    let mut summary: Option<String> = None;

    for line in output.lines() {
        if line.contains("file") && (line.contains("reformatted") || line.contains("unchanged")) {
            summary = Some(line.to_string());
        } else if line.trim().starts_with("Reformatted ") {
            reformatted += 1;
        }
    }

    if let Some(s) = summary {
        return s;
    }
    if reformatted > 0 {
        return format!("{} files reformatted", reformatted);
    }
    output.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ruff_clean_returns_ok() {
        let handler = RuffHandler;
        let result = handler.filter("All checks passed.\n", &["ruff".to_string(), "check".to_string()]);
        assert_eq!(result.trim(), "ruff: ok");
    }

    #[test]
    fn ruff_empty_output_returns_ok() {
        let handler = RuffHandler;
        let result = handler.filter("", &["ruff".to_string(), "check".to_string()]);
        assert_eq!(result.trim(), "ruff: ok");
    }

    #[test]
    fn ruff_groups_violations_by_code() {
        let input = "src/foo.py:1:1: E501 Line too long (100 > 88)\nsrc/bar.py:2:1: E501 Line too long (95 > 88)\nsrc/baz.py:3:1: E302 Expected 2 blank lines\n";
        let handler = RuffHandler;
        let result = handler.filter(input, &["ruff".to_string(), "check".to_string()]);
        assert!(result.contains("E501 ×2") || result.contains("[E501"), "E501 not grouped");
        assert!(result.contains("E302"), "E302 missing");
    }

    #[test]
    fn ruff_shows_n_more_for_many_occurrences() {
        let input: String = (1..=6)
            .map(|i| format!("src/f{}.py:{}:1: W291 Trailing whitespace\n", i, i))
            .collect();
        let handler = RuffHandler;
        let result = handler.filter(&input, &["ruff".to_string(), "check".to_string()]);
        assert!(result.contains("×6") || result.contains("more"), "no grouping for 6 occurrences");
    }

    #[test]
    fn ruff_format_returns_summary() {
        let handler = RuffHandler;
        let input = "Reformatted src/a.py\nReformatted src/b.py\n2 files reformatted, 5 files left unchanged\n";
        let result = handler.filter(input, &["ruff".to_string(), "format".to_string()]);
        assert!(result.contains("reformatted"), "summary not returned");
    }

    #[test]
    fn ruff_rewrite_args_injects_output_format() {
        let handler = RuffHandler;
        let args = vec!["ruff".to_string(), "check".to_string(), ".".to_string()];
        let result = handler.rewrite_args(&args);
        assert!(result.contains(&"--output-format".to_string()), "format flag not injected");
        assert!(result.contains(&"concise".to_string()));
    }

    #[test]
    fn ruff_rewrite_args_no_duplicate() {
        let handler = RuffHandler;
        let args = vec!["ruff".to_string(), "check".to_string(), "--output-format".to_string(), "json".to_string()];
        let result = handler.rewrite_args(&args);
        let count = result.iter().filter(|a| a.contains("--output-format")).count();
        assert_eq!(count, 1, "duplicate --output-format injected");
    }
}
