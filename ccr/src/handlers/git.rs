use super::util;
use super::Handler;

pub struct GitHandler;

const PUSH_PULL_RULES: &[util::MatchOutputRule] = &[util::MatchOutputRule {
    success_pattern: r"(?i)Everything up-to-date|Already up to date",
    error_pattern: r"(?i)error:|rejected|conflict|denied",
    ok_message: "ok (up to date)",
}];

impl Handler for GitHandler {
    fn rewrite_args(&self, args: &[String]) -> Vec<String> {
        let subcmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
        match subcmd {
            "log" => {
                if !args.iter().any(|a| a == "--oneline") {
                    let mut out = args.to_vec();
                    out.insert(2, "--oneline".to_string());
                    return out;
                }
            }
            "status" => {
                // Inject --porcelain so filter_status always receives XY format
                if !args.iter().any(|a| a == "--porcelain" || a == "--short" || a == "-s") {
                    let mut out = args.to_vec();
                    out.insert(2, "--porcelain".to_string());
                    return out;
                }
            }
            _ => {}
        }
        args.to_vec()
    }

    fn filter(&self, output: &str, args: &[String]) -> String {
        let subcmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
        match subcmd {
            "status" => filter_status(output),
            "log" => filter_log(output),
            "diff" => filter_diff(output),
            "push" | "pull" | "fetch" => filter_push_pull(output),
            "commit" | "add" => filter_commit(output),
            "branch" | "stash" => filter_list(output),
            _ => output.to_string(),
        }
    }
}

fn filter_status(output: &str) -> String {
    // Short-circuit for clean working tree
    if output.contains("nothing to commit") || output.trim().is_empty() {
        return "nothing to commit, working tree clean".to_string();
    }

    let mut staged: Vec<String> = Vec::new();
    let mut modified: Vec<String> = Vec::new();
    let mut untracked: Vec<String> = Vec::new();
    let mut in_untracked_section = false;

    for line in output.lines() {
        // Detect the untracked files section header
        if line.trim().starts_with("Untracked files:") {
            in_untracked_section = true;
            continue;
        }
        // Reset untracked section if we hit another section header (blank line followed by text)
        if line.trim().is_empty() {
            continue;
        }
        // Skip git hint lines
        if line.trim().starts_with("(use \"git") || line.trim().starts_with("no changes added") {
            continue;
        }

        // Porcelain v1 format: XY filename
        // X = index status, Y = worktree status
        // "?? file" = untracked
        if line.starts_with("??") {
            let name = line[3..].trim().to_string();
            if !name.is_empty() {
                untracked.push(name);
            }
            continue;
        }

        // Lines in the untracked section (non-porcelain output)
        if in_untracked_section {
            let t = line.trim();
            if !t.is_empty() && !t.starts_with("(use \"git") {
                untracked.push(t.to_string());
            }
            continue;
        }

        // Staged changes: index status is non-space (first char) in "XY filename"
        // e.g. "M  file", "A  file", "D  file", "R  old -> new"
        // Modified/unstaged: worktree status is non-space (second char), index is space
        // e.g. " M file", " D file"
        if line.len() >= 2 {
            let x = line.chars().next().unwrap_or(' ');
            let y = line.chars().nth(1).unwrap_or(' ');
            let rest = line.get(3..).unwrap_or("").trim().to_string();
            if rest.is_empty() {
                continue;
            }
            if x != ' ' && x != '#' {
                staged.push(rest);
            } else if y != ' ' {
                modified.push(rest);
            }
        }
    }

    // If we found nothing categorized, fall back to branch line pass-through
    if staged.is_empty() && modified.is_empty() && untracked.is_empty() {
        return "nothing to commit, working tree clean".to_string();
    }

    let mut out: Vec<String> = Vec::new();

    // Summary header
    out.push(format!(
        "Staged: {} · Modified: {} · Untracked: {}",
        staged.len(),
        modified.len(),
        untracked.len()
    ));

    // List staged + modified (max 15 combined)
    const MAX_STAGED_MODIFIED: usize = 15;
    let sm_combined: Vec<&String> = staged.iter().chain(modified.iter()).collect();
    let sm_shown = MAX_STAGED_MODIFIED.min(sm_combined.len());
    for entry in &sm_combined[..sm_shown] {
        out.push(format!("  {}", entry));
    }
    let sm_extra = sm_combined.len().saturating_sub(sm_shown);
    if sm_extra > 0 {
        out.push(format!("[+{} more staged/modified]", sm_extra));
    }

    // List untracked (max 10)
    const MAX_UNTRACKED: usize = 10;
    let ut_shown = MAX_UNTRACKED.min(untracked.len());
    for entry in &untracked[..ut_shown] {
        out.push(format!("  {}", entry));
    }
    let ut_extra = untracked.len().saturating_sub(ut_shown);
    if ut_extra > 0 {
        out.push(format!("[+{} more untracked]", ut_extra));
    }

    out.join("\n")
}

fn filter_log(output: &str) -> String {
    // With --oneline, each line is "hash message"; truncate long lines
    let lines: Vec<String> = output
        .lines()
        .take(20)
        .map(|l| {
            let chars: Vec<char> = l.chars().collect();
            if chars.len() > 100 {
                format!("{}…", chars[..99].iter().collect::<String>())
            } else {
                l.to_string()
            }
        })
        .collect();
    let total = output.lines().count();
    let mut out = lines.join("\n");
    if total > 20 {
        out.push_str(&format!("\n[+{} more commits]", total - 20));
    }
    out
}

fn filter_diff(output: &str) -> String {
    // Keep: diff/---/+++/@@/+/- lines plus up to 2 context lines after each change block.
    // This keeps just enough context to locate the change without the full 3-line default.
    let lines: Vec<&str> = output.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut context_remaining: usize = 0;

    for line in &lines {
        let is_structural = line.starts_with("diff ")
            || line.starts_with("index ")
            || line.starts_with("---")
            || line.starts_with("+++")
            || line.starts_with("@@");
        let is_change = line.starts_with('+') || line.starts_with('-');
        let is_context = line.starts_with(' ');

        if is_structural || is_change {
            out.push(line.to_string());
            if is_change {
                context_remaining = 2; // allow up to 2 context lines after a change
            }
        } else if is_context && context_remaining > 0 {
            out.push(line.to_string());
            context_remaining -= 1;
        } else {
            context_remaining = 0;
        }
    }

    if out.is_empty() {
        output.to_string()
    } else {
        out.join("\n")
    }
}

fn filter_push_pull(output: &str) -> String {
    // Short-circuit: known terminal outcomes
    if let Some(msg) = util::check_match_output(output, PUSH_PULL_RULES) {
        return msg;
    }

    let mut out: Vec<String> = Vec::new();

    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        // Skip pure noise lines
        if t.starts_with("remote: Counting")
            || t.starts_with("remote: Compressing")
            || t.starts_with("remote: Enumerating")
            || t.starts_with("Counting objects")
            || t.starts_with("Compressing objects")
            || t.starts_with("Writing objects")
            || t.starts_with("Delta compression")
            || t.starts_with("remote: Total")
            || t.starts_with("Resolving deltas")
        {
            continue;
        }

        // Keep branch tracking lines: "branch -> remote/branch"
        if t.contains(" -> ") {
            out.push(t.to_string());
            continue;
        }

        // Keep file change summary lines
        if t.contains("file") && (t.contains("changed") || t.contains("insertion") || t.contains("deletion")) {
            out.push(t.to_string());
            continue;
        }

        // Keep error/warning lines
        if t.contains("error") || t.contains("rejected") || t.contains("conflict") || t.contains("denied") {
            out.push(t.to_string());
            continue;
        }

        // Keep other non-noise lines
        out.push(t.to_string());
    }

    if out.is_empty() {
        output.to_string()
    } else {
        out.join("\n")
    }
}

fn filter_commit(output: &str) -> String {
    // Return "ok — [branch abc1234] message\nN files changed, +X -Y" format
    let mut bracket_line: Option<String> = None;
    let mut stats_line: Option<String> = None;

    for line in output.lines() {
        let t = line.trim();
        if t.starts_with('[') && bracket_line.is_none() {
            bracket_line = Some(t.to_string());
        }
        if t.contains("file") && (t.contains("changed") || t.contains("insertion") || t.contains("deletion")) {
            stats_line = Some(t.to_string());
        }
    }

    match (bracket_line, stats_line) {
        (Some(b), Some(s)) => format!("ok — {}\n{}", b, s),
        (Some(b), None) => format!("ok — {}", b),
        _ => output.to_string(),
    }
}

fn filter_list(output: &str) -> String {
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() > 30 {
        let shown = &lines[..30];
        let extra = lines.len() - 30;
        let mut out: Vec<String> = shown.iter().map(|l| l.to_string()).collect();
        out.push(format!("[+{} more]", extra));
        out.join("\n")
    } else {
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rewrite_injects_porcelain() {
        let handler = GitHandler;
        let args: Vec<String> = vec!["git".into(), "status".into()];
        let rewritten = handler.rewrite_args(&args);
        assert!(rewritten.contains(&"--porcelain".to_string()), "should inject --porcelain");
    }

    #[test]
    fn test_rewrite_no_double_porcelain() {
        let handler = GitHandler;
        let args: Vec<String> = vec!["git".into(), "status".into(), "--porcelain".into()];
        let rewritten = handler.rewrite_args(&args);
        assert_eq!(rewritten.iter().filter(|a| *a == "--porcelain").count(), 1);
    }

    #[test]
    fn test_diff_keeps_context_lines() {
        let output = "diff --git a/foo.rs b/foo.rs\n--- a/foo.rs\n+++ b/foo.rs\n@@ -1,5 +1,5 @@\n fn main() {\n-    old();\n+    new();\n     println!(\"done\");\n     let x = 1;\n     let y = 2;";
        let result = filter_diff(output);
        assert!(result.contains("+    new();"), "change lines kept");
        assert!(result.contains("-    old();"), "change lines kept");
        assert!(result.contains("println!"), "first context line after change kept");
        assert!(result.contains("let x = 1"), "second context line after change kept");
        assert!(!result.contains("let y = 2"), "third context line should be stripped");
    }

    #[test]
    fn test_status_clean() {
        let output = "On branch main\nnothing to commit, working tree clean\n";
        assert_eq!(filter_status(output), "nothing to commit, working tree clean");
    }

    #[test]
    fn test_status_empty() {
        assert_eq!(filter_status(""), "nothing to commit, working tree clean");
    }

    #[test]
    fn test_status_staged_and_untracked() {
        // Porcelain v1 format
        let output = "M  src/main.rs\nA  src/new.rs\n?? untracked.txt\n?? other.txt\n";
        let result = filter_status(output);
        assert!(result.contains("Staged: 2"), "expected Staged: 2, got: {}", result);
        assert!(result.contains("Modified: 0"), "expected Modified: 0, got: {}", result);
        assert!(result.contains("Untracked: 2"), "expected Untracked: 2, got: {}", result);
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("untracked.txt"));
    }

    #[test]
    fn test_status_modified_unstaged() {
        let output = " M src/lib.rs\n?? foo.txt\n";
        let result = filter_status(output);
        assert!(result.contains("Modified: 1"), "got: {}", result);
        assert!(result.contains("Untracked: 1"), "got: {}", result);
    }

    #[test]
    fn test_push_pull_up_to_date() {
        let output = "Everything up-to-date\n";
        assert_eq!(filter_push_pull(output), "ok (up to date)");
    }

    #[test]
    fn test_push_pull_already_up_to_date() {
        let output = "Already up to date.\n";
        assert_eq!(filter_push_pull(output), "ok (up to date)");
    }

    #[test]
    fn test_push_pull_error_not_short_circuited() {
        let output = "Everything up-to-date\nerror: failed to push some refs\n";
        let result = filter_push_pull(output);
        // Should NOT short-circuit because error pattern matches
        assert_ne!(result, "ok (up to date)");
        assert!(result.contains("error"));
    }

    #[test]
    fn test_push_pull_with_branch_tracking() {
        let output = "remote: Counting objects: 3\nmain -> origin/main\n3 files changed, 42 insertions(+), 17 deletions(-)\n";
        let result = filter_push_pull(output);
        assert!(result.contains("main -> origin/main"), "got: {}", result);
        assert!(!result.contains("remote: Counting"), "noise not filtered, got: {}", result);
    }

    #[test]
    fn test_commit_format() {
        let output = "[main abc1234] Add feature\n 2 files changed, 10 insertions(+), 3 deletions(-)\n";
        let result = filter_commit(output);
        assert!(result.starts_with("ok — [main abc1234]"), "got: {}", result);
        assert!(result.contains("2 files changed"), "got: {}", result);
    }
}
