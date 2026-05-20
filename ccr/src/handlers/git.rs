use std::collections::HashSet;

use super::Handler;

pub struct GitHandler;

/// Find the git subcommand, skipping any global options that appear before it.
///
/// Examples:
/// - `["git", "status"]`            → "status"
/// - `["git", "-C", "/path", "log"]`→ "log"
/// - `["git", "--no-pager", "diff"]`→ "diff"
/// - `["git", "-c", "k=v", "push"]` → "push"
fn git_subcmd(args: &[String]) -> &str {
    let mut i = 1usize; // skip argv[0] = "git"
    while i < args.len() {
        let a = args[i].as_str();
        // Options that consume the next argument as their value
        if matches!(a, "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace" | "--super-prefix") {
            i += 2;
            continue;
        }
        // Options that embed their value or are standalone boolean flags
        if a.starts_with("--git-dir=")
            || a.starts_with("--work-tree=")
            || a.starts_with("--namespace=")
            || a.starts_with("-c=")
            || matches!(
                a,
                "--no-pager" | "--paginate" | "-p"
                | "--bare" | "--no-replace-objects"
                | "--literal-pathspecs" | "--no-optional-locks"
                | "--version" | "--help"
            )
        {
            i += 1;
            continue;
        }
        // First non-option token is the subcommand
        if !a.starts_with('-') {
            return a;
        }
        // Unknown option — skip
        i += 1;
    }
    ""
}

const PUSH_PULL_ERROR_TERMS: &[&str] = &["error:", "rejected", "conflict", "denied", "fatal:"];

impl Handler for GitHandler {
    fn rewrite_args(&self, args: &[String]) -> Vec<String> {
        let subcmd = git_subcmd(args);
        match subcmd {
            "log" => {
                let mut out = args.to_vec();
                let subcmd_pos = args.iter().position(|a| a.as_str() == subcmd).unwrap_or(1);
                // Insert flags after subcmd position, in reverse order so they end up
                // as: log --oneline --graph --decorate
                let mut insert_pos = subcmd_pos + 1;
                if !out.iter().any(|a| a == "--oneline") {
                    out.insert(insert_pos, "--oneline".to_string());
                    insert_pos += 1;
                }
                if !out.iter().any(|a| a == "--graph") {
                    out.insert(insert_pos, "--graph".to_string());
                    insert_pos += 1;
                }
                if !out.iter().any(|a| a == "--decorate") {
                    out.insert(insert_pos, "--decorate".to_string());
                }
                if out != args {
                    return out;
                }
            }
            "status" => {
                if !args.iter().any(|a| a == "--porcelain" || a == "--short" || a == "-s") {
                    let mut out = args.to_vec();
                    let subcmd_pos = args.iter().position(|a| a.as_str() == subcmd).unwrap_or(1);
                    out.insert(subcmd_pos + 1, "--porcelain".to_string());
                    return out;
                }
            }
            _ => {}
        }
        args.to_vec()
    }

    fn filter(&self, output: &str, args: &[String]) -> String {
        let subcmd = git_subcmd(args);
        match subcmd {
            "status" => filter_status(output),
            "log" => filter_log(output),
            "diff" => filter_diff(output),
            "push" | "pull" | "fetch" => filter_push_pull(output),
            "commit" | "add" => filter_commit(output),
            "branch" | "stash" => filter_list(output),
            "merge" => filter_merge(output),
            "rebase" => filter_rebase(output),
            "clone" => filter_clone(output),
            "checkout" | "switch" => filter_checkout(output),
            _ => output.to_string(),
        }
    }
}

// ─── status ──────────────────────────────────────────────────────────────────

fn filter_status(output: &str) -> String {
    if output.contains("nothing to commit") || output.trim().is_empty() {
        return "nothing to commit, working tree clean".to_string();
    }

    let mut staged: Vec<String> = Vec::new();
    let mut modified: Vec<String> = Vec::new();
    let mut untracked: Vec<String> = Vec::new();

    for line in output.lines() {
        if line.trim().is_empty()
            || line.trim().starts_with("(use \"git")
            || line.trim().starts_with("no changes added")
        {
            continue;
        }
        if line.len() < 2 {
            continue;
        }

        let x = line.chars().next().unwrap_or(' ');
        let y = line.chars().nth(1).unwrap_or(' ');

        if x == '?' && y == '?' {
            let name = line.get(3..).unwrap_or("").trim().to_string();
            if !name.is_empty() {
                untracked.push(name);
            }
            continue;
        }

        let rest = line.get(3..).unwrap_or("").trim().to_string();
        if rest.is_empty() {
            continue;
        }
        if x != ' ' && x != '#' {
            staged.push(rest.clone());
        }
        if y != ' ' && y != '#' {
            modified.push(rest);
        }
    }

    if staged.is_empty() && modified.is_empty() && untracked.is_empty() {
        return "nothing to commit, working tree clean".to_string();
    }

    let mut out: Vec<String> = Vec::new();

    out.push(format!(
        "Staged: {} · Modified: {} · Untracked: {}",
        staged.len(),
        modified.len(),
        untracked.len()
    ));

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

// ─── log ─────────────────────────────────────────────────────────────────────

/// Trailer prefixes stripped from one-line commit subjects.
const TRAILERS: &[&str] = &[
    "Signed-off-by:", "Co-authored-by:", "Change-Id:", "Reviewed-by:",
    "Acked-by:", "Tested-by:", "Reported-by:", "Cc:",
];

fn log_line_cap() -> usize {
    crate::config_loader::load_config()
        .map(|c| c.global.git_log_limit)
        .unwrap_or(25)
}

fn filter_log(output: &str) -> String {
    let cap = log_line_cap();

    let lines: Vec<&str> = output
        .lines()
        .filter(|l| {
            let msg = l.splitn(2, ' ').nth(1).unwrap_or("");
            !TRAILERS.iter().any(|t| msg.trim_start().starts_with(t))
        })
        .take(cap)
        .collect();

    let total = output.lines().count();
    let mut result: Vec<String> = lines
        .iter()
        .map(|l| {
            let chars: Vec<char> = l.chars().collect();
            if chars.len() > 100 {
                format!("{}…", chars[..99].iter().collect::<String>())
            } else {
                l.to_string()
            }
        })
        .collect();

    if total > cap {
        result.push(format!("[+{} more commits, {} total]", total - cap, total));
    }
    result.join("\n")
}

// ─── diff ────────────────────────────────────────────────────────────────────

/// Hard cap per hunk and across the whole diff.
const HUNK_LINE_CAP: usize = 20;
/// Total line budget for the whole diff output.
/// Kept below BERT_MIN_LINES (≈15 tokens) for typical small diffs so BERT is skipped.
/// Large diffs still trigger BERT but with a much smaller input.
const DIFF_TOTAL_CAP: usize = 60;
/// Maximum context lines kept on each side of a changed block.
const MAX_CONTEXT_PER_SIDE: usize = 2;
/// Maximum character length for a single +/- diff line before truncation.
const DIFF_LINE_CHAR_CAP: usize = 120;
/// Jaccard similarity threshold: pairs above this are counted as ~modified.
const JACCARD_THRESHOLD: f64 = 0.5;

/// Compute Jaccard similarity between two strings by splitting on whitespace.
/// Returns a value in [0.0, 1.0] where 1.0 means identical token sets.
fn jaccard_similarity(a: &str, b: &str) -> f64 {
    let set_a: HashSet<&str> = a.split_whitespace().collect();
    let set_b: HashSet<&str> = b.split_whitespace().collect();
    if set_a.is_empty() && set_b.is_empty() {
        return 1.0;
    }
    let intersection = set_a.intersection(&set_b).count();
    let union = set_a.union(&set_b).count();
    if union == 0 {
        return 0.0;
    }
    intersection as f64 / union as f64
}

/// Given buffered removed and added lines from a contiguous change block,
/// greedily pair them by position and classify via Jaccard similarity.
/// Returns (pure_added, pure_removed, modified) counts.
fn classify_changes(removed: &[&str], added: &[&str]) -> (usize, usize, usize) {
    let paired = removed.len().min(added.len());
    let mut modified = 0usize;
    let mut dissimilar_pairs = 0usize;

    for i in 0..paired {
        // Strip the leading +/- prefix before comparing content
        let r = removed[i].get(1..).unwrap_or("");
        let a = added[i].get(1..).unwrap_or("");
        if jaccard_similarity(r, a) >= JACCARD_THRESHOLD {
            modified += 1;
        } else {
            dissimilar_pairs += 1;
        }
    }

    // Dissimilar pairs count as both a removal and an addition (not modifications).
    // Unpaired extras are pure additions or removals.
    let pure_removed = dissimilar_pairs + removed.len().saturating_sub(paired);
    let pure_added = dissimilar_pairs + added.len().saturating_sub(paired);
    (pure_added, pure_removed, modified)
}

/// Truncate a diff line to DIFF_LINE_CHAR_CAP characters, preserving the +/- prefix.
fn truncate_diff_line(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    if chars.len() > DIFF_LINE_CHAR_CAP {
        format!("{}…", chars[..DIFF_LINE_CHAR_CAP - 1].iter().collect::<String>())
    } else {
        line.to_string()
    }
}

fn filter_diff(output: &str) -> String {
    if crate::handlers::util::mid_git_operation() {
        return output.to_string();
    }
    let lines: Vec<&str> = output.lines().collect();
    let mut out: Vec<String> = Vec::new();

    // Per-file change tally (flushed when a new file starts or at end).
    // The macro resets these on every flush; the final flush's resets are dead stores.
    #[allow(unused_assignments)]
    let mut file_header_idx: Option<usize> = None;
    #[allow(unused_assignments)]
    let mut file_added: usize = 0;
    #[allow(unused_assignments)]
    let mut file_removed: usize = 0;
    #[allow(unused_assignments)]
    let mut file_modified: usize = 0;

    let mut hunk_lines: usize = 0;
    let mut hunk_truncated = false;
    let mut total_lines: usize = 0;
    let mut global_truncated = false;

    // Context trimming state
    let mut ctx_after: usize = 0;
    let mut ctx_pending: Vec<String> = Vec::new();

    // Change block buffer: accumulate consecutive -/+ lines, then classify.
    let mut block_removed: Vec<&str> = Vec::new();
    let mut block_added: Vec<&str> = Vec::new();

    // Flush the per-file tally into the file header line.
    macro_rules! flush_file_tally {
        () => {
            if let Some(idx) = file_header_idx {
                if file_added > 0 || file_removed > 0 || file_modified > 0 {
                    let mut parts = Vec::new();
                    if file_added > 0 { parts.push(format!("+{}", file_added)); }
                    if file_removed > 0 { parts.push(format!("-{}", file_removed)); }
                    if file_modified > 0 { parts.push(format!("~{}", file_modified)); }
                    out[idx] = format!("{} [{}]", out[idx], parts.join(" "));
                }
                file_header_idx = None;
                file_added = 0;
                file_removed = 0;
                file_modified = 0;
            }
        };
    }

    // Flush a buffered change block: classify via Jaccard, update tallies.
    macro_rules! flush_change_block {
        () => {
            if !block_removed.is_empty() || !block_added.is_empty() {
                let (a, r, m) = classify_changes(&block_removed, &block_added);
                file_added += a;
                file_removed += r;
                file_modified += m;
                block_removed.clear();
                block_added.clear();
            }
        };
    }

    for line in &lines {
        // Even after global truncation, keep counting for tallies
        if global_truncated {
            if line.starts_with("diff --git ") {
                flush_change_block!();
                flush_file_tally!();
                let fname = line
                    .split_whitespace()
                    .last()
                    .and_then(|s| s.strip_prefix("b/"))
                    .unwrap_or(line);
                file_header_idx = Some(out.len());
                out.push(fname.to_string());
                // Don't bump total_lines — we're in the truncated tail
            } else if line.starts_with('+') && !line.starts_with("+++") {
                block_added.push(line);
            } else if line.starts_with('-') && !line.starts_with("---") {
                block_removed.push(line);
            } else if !line.starts_with('+') && !line.starts_with('-') {
                flush_change_block!();
            }
            continue;
        }

        if line.starts_with("diff --git ") {
            flush_change_block!();
            ctx_pending.clear();
            ctx_after = 0;
            flush_file_tally!();

            let fname = line
                .split_whitespace()
                .last()
                .and_then(|s| s.strip_prefix("b/"))
                .unwrap_or(line);
            file_header_idx = Some(out.len());
            out.push(fname.to_string());
            total_lines += 1;
            hunk_lines = 0;
            hunk_truncated = false;
            continue;
        }

        // Drop noisy headers
        if line.starts_with("--- ")
            || line.starts_with("+++ ")
            || line.starts_with("index ")
            || line.starts_with("\\ No newline")
        {
            continue;
        }

        // Hunk header: reset per-hunk state
        if line.starts_with("@@") {
            flush_change_block!();
            ctx_pending.clear();
            ctx_after = 0;
            hunk_lines = 0;
            hunk_truncated = false;
            out.push(hunk_context(line));
            total_lines += 1;
            continue;
        }

        // Context lines (' '): flush any pending change block first
        if line.starts_with(' ') {
            flush_change_block!();
            if hunk_truncated {
                continue;
            }
            if ctx_after < MAX_CONTEXT_PER_SIDE {
                out.push(line.to_string());
                hunk_lines += 1;
                total_lines += 1;
                ctx_after += 1;
                if total_lines >= DIFF_TOTAL_CAP {
                    global_truncated = true;
                }
            } else {
                ctx_pending.push(line.to_string());
            }
            continue;
        }

        // Changed lines ('+'/'-')
        if line.starts_with('+') || line.starts_with('-') {
            // Always buffer for Jaccard classification
            if line.starts_with('-') {
                block_removed.push(line);
            } else {
                block_added.push(line);
            }

            if hunk_truncated {
                continue;
            }

            // Flush leading context from pending buffer
            if !ctx_pending.is_empty() {
                let skip = ctx_pending.len().saturating_sub(MAX_CONTEXT_PER_SIDE);
                for ctx_line in ctx_pending.drain(skip..) {
                    if !global_truncated {
                        out.push(ctx_line);
                        hunk_lines += 1;
                        total_lines += 1;
                        if total_lines >= DIFF_TOTAL_CAP {
                            global_truncated = true;
                        }
                    }
                }
                ctx_pending.clear();
            }
            ctx_after = 0;

            if hunk_lines >= HUNK_LINE_CAP {
                hunk_truncated = true;
                out.push("  [...truncated...]".to_string());
                total_lines += 1;
            } else if !global_truncated {
                out.push(truncate_diff_line(line));
                hunk_lines += 1;
                total_lines += 1;
                if total_lines >= DIFF_TOTAL_CAP {
                    global_truncated = true;
                }
            }
        }
    }

    // Flush remaining state
    flush_change_block!();
    flush_file_tally!();

    if global_truncated {
        out.push("[... diff truncated — run `git diff` for full output]".to_string());
    }

    if out.is_empty() {
        output.to_string()
    } else {
        out.join("\n")
    }
}

/// Extract the human-readable function/class context from a `@@ ... @@ context` line.
fn hunk_context(header: &str) -> String {
    // "@@ -L,N +L,N @@ fn foo() {" → "@@ fn foo() {"
    let parts: Vec<&str> = header.splitn(4, "@@").collect();
    if parts.len() >= 3 {
        let ctx = parts[2].trim();
        if !ctx.is_empty() {
            return format!("@@ {}", ctx);
        }
    }
    "@@".to_string()
}

// ─── push / pull / fetch ─────────────────────────────────────────────────────

fn filter_push_pull(output: &str) -> String {
    let has_error = output.lines().any(|l| {
        let t = l.trim().to_lowercase();
        PUSH_PULL_ERROR_TERMS.iter().any(|e| t.contains(e))
    });

    // Already up to date (only if no errors)
    if !has_error && (output.contains("Everything up-to-date") || output.contains("Already up to date")) {
        return "ok (up to date)".to_string();
    }

    if has_error {
        let lines: Vec<&str> = output.lines().collect();
        let n = lines.len();
        let mut keep = vec![false; n];
        for (i, line) in lines.iter().enumerate() {
            let t = line.trim().to_lowercase();
            if PUSH_PULL_ERROR_TERMS.iter().any(|e| t.contains(e)) {
                let start = i.saturating_sub(2);
                let end = (i + 2).min(n.saturating_sub(1));
                for j in start..=end {
                    keep[j] = true;
                }
            }
        }
        let mut result: Vec<String> = Vec::new();
        let mut last_kept: Option<usize> = None;
        for (i, &k) in keep.iter().enumerate() {
            if k {
                if let Some(last) = last_kept {
                    if last + 1 < i {
                        result.push("...".to_string());
                    }
                }
                result.push(lines[i].to_string());
                last_kept = Some(i);
            }
        }
        return result.join("\n");
    }

    // Success — find the branch ref line: "main -> origin/main" or "branch 'main' set up to track..."
    for line in output.lines() {
        let t = line.trim();
        if t.contains(" -> ") && !t.starts_with("remote:") {
            return format!("ok {}", t);
        }
    }

    // Pull / fetch with file stats
    for line in output.lines() {
        let t = line.trim();
        if t.contains("file") && (t.contains("changed") || t.contains("insertion") || t.contains("deletion")) {
            return format!("ok ({})", t);
        }
    }

    // Fallback: last meaningful line
    output
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(|l| format!("ok {}", l.trim()))
        .unwrap_or_else(|| "ok".to_string())
}

// ─── commit / add ────────────────────────────────────────────────────────────

fn filter_commit(output: &str) -> String {
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

// ─── branch / stash ──────────────────────────────────────────────────────────

fn filter_list(output: &str) -> String {
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() > 30 {
        let extra = lines.len() - 30;
        let mut out: Vec<String> = lines[..30].iter().map(|l| l.to_string()).collect();
        out.push(format!("[+{} more]", extra));
        out.join("\n")
    } else {
        lines.join("\n")
    }
}

// ─── merge ───────────────────────────────────────────────────────────────────

fn filter_merge(output: &str) -> String {
    if output.contains("CONFLICT") {
        let mut out: Vec<String> = Vec::new();
        for line in output.lines() {
            let t = line.trim();
            if t.contains("CONFLICT") || t.contains("Automatic merge failed") {
                out.push(line.to_string());
            }
        }
        return if out.is_empty() { output.to_string() } else { out.join("\n") };
    }
    if output.contains("Already up to date") {
        return "ok (already up to date)".to_string();
    }
    if output.contains("Fast-forward") {
        return "ok (fast-forward)".to_string();
    }
    // Keep "Merge made by" line + diffstat line
    let mut out: Vec<String> = Vec::new();
    for line in output.lines() {
        let t = line.trim();
        if t.starts_with("Merge made by")
            || (t.contains("file") && (t.contains("changed") || t.contains("insertion") || t.contains("deletion")))
        {
            out.push(line.to_string());
        }
    }
    if out.is_empty() { output.to_string() } else { out.join("\n") }
}

// ─── rebase ──────────────────────────────────────────────────────────────────

fn filter_rebase(output: &str) -> String {
    if output.contains("CONFLICT") {
        let mut out: Vec<String> = Vec::new();
        for line in output.lines() {
            let t = line.trim();
            if t.contains("CONFLICT") || t.contains("could not apply") {
                out.push(line.to_string());
            }
        }
        return if out.is_empty() { output.to_string() } else { out.join("\n") };
    }
    if output.contains("Successfully rebased") {
        for line in output.lines() {
            if line.contains("Successfully rebased") {
                return format!("ok — {}", line.trim());
            }
        }
        return "ok — rebased".to_string();
    }
    if output.contains("is up to date") {
        return "ok (already up to date)".to_string();
    }
    // Return last non-empty line
    output
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim().to_string())
        .unwrap_or_else(|| output.to_string())
}

// ─── clone ───────────────────────────────────────────────────────────────────

const CLONE_NOISE_PREFIXES: &[&str] = &[
    "remote: Enumerating", "remote: Counting", "remote: Compressing",
    "Receiving objects:", "Resolving deltas:", "Checking connectivity",
];

fn filter_clone(output: &str) -> String {
    let mut cloned_dir: Option<String> = None;
    let mut error_lines: Vec<String> = Vec::new();

    for line in output.lines() {
        let t = line.trim();
        if CLONE_NOISE_PREFIXES.iter().any(|p| t.starts_with(p)) {
            continue;
        }
        if t.starts_with("Cloning into") {
            if let Some(start) = t.find('\'') {
                if let Some(end) = t[start + 1..].find('\'') {
                    let dir = &t[start + 1..start + 1 + end];
                    let dir_name = dir.split('/').last().unwrap_or(dir);
                    cloned_dir = Some(dir_name.to_string());
                }
            }
            continue;
        }
        if t.contains("error:") || t.contains("fatal:") || t.starts_with("ERROR") {
            error_lines.push(line.to_string());
        }
    }

    if !error_lines.is_empty() {
        return error_lines.join("\n");
    }
    if let Some(dir) = cloned_dir {
        return format!("ok — cloned '{}'", dir);
    }
    output.to_string()
}

// ─── checkout / switch ───────────────────────────────────────────────────────

fn filter_checkout(output: &str) -> String {
    // Check for errors first
    let has_error = output.lines().any(|l| {
        let t = l.trim();
        t.contains("error:") || t.contains("Your local changes") || t.contains("Please commit")
    });
    if has_error {
        return output.to_string();
    }

    for line in output.lines() {
        let t = line.trim();
        if t.starts_with("Switched to a new branch") {
            if let (Some(s), Some(e)) = (t.find('\''), t.rfind('\'')) {
                if s < e {
                    let branch = &t[s + 1..e];
                    return format!("ok — new branch '{}'", branch);
                }
            }
            return format!("ok — {}", t);
        }
        if t.starts_with("Switched to branch") {
            if let (Some(s), Some(e)) = (t.find('\''), t.rfind('\'')) {
                if s < e {
                    let branch = &t[s + 1..e];
                    return format!("ok — switched to '{}'", branch);
                }
            }
            return format!("ok — {}", t);
        }
        if t.starts_with("Already on") {
            if let (Some(s), Some(e)) = (t.find('\''), t.rfind('\'')) {
                if s < e {
                    let branch = &t[s + 1..e];
                    return format!("ok (already on '{}')", branch);
                }
            }
            return "ok (already on branch)".to_string();
        }
    }

    // Fallback: last non-empty line
    output
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim().to_string())
        .unwrap_or_else(|| output.to_string())
}

// ─── tests ───────────────────────────────────────────────────────────────────

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
    fn test_rewrite_with_dash_c_global_option() {
        let handler = GitHandler;
        let args: Vec<String> = vec!["git".into(), "-C".into(), "/some/repo".into(), "status".into()];
        let rewritten = handler.rewrite_args(&args);
        assert!(rewritten.contains(&"--porcelain".to_string()),
            "should inject --porcelain even with -C global option: {:?}", rewritten);
    }

    #[test]
    fn test_rewrite_with_no_pager_global_option() {
        let handler = GitHandler;
        let args: Vec<String> = vec!["git".into(), "--no-pager".into(), "log".into()];
        let rewritten = handler.rewrite_args(&args);
        assert!(rewritten.contains(&"--oneline".to_string()),
            "should inject --oneline even with --no-pager: {:?}", rewritten);
        assert!(rewritten.contains(&"--graph".to_string()),
            "should inject --graph: {:?}", rewritten);
        assert!(rewritten.contains(&"--decorate".to_string()),
            "should inject --decorate: {:?}", rewritten);
    }

    #[test]
    fn test_rewrite_log_no_double_graph_decorate() {
        let handler = GitHandler;
        let args: Vec<String> = vec![
            "git".into(), "log".into(), "--oneline".into(),
            "--graph".into(), "--decorate".into(),
        ];
        let rewritten = handler.rewrite_args(&args);
        assert_eq!(rewritten.iter().filter(|a| *a == "--graph").count(), 1,
            "should not double --graph: {:?}", rewritten);
        assert_eq!(rewritten.iter().filter(|a| *a == "--decorate").count(), 1,
            "should not double --decorate: {:?}", rewritten);
    }

    #[test]
    fn test_filter_with_dash_c_global_option() {
        let handler = GitHandler;
        let args: Vec<String> = vec!["git".into(), "-C".into(), "/path".into(), "push".into()];
        // push filter should not passthrough raw (it should use filter_push_pull)
        let output = "Everything up-to-date\n";
        let result = handler.filter(output, &args);
        assert_eq!(result, "ok (up to date)", "filter should route correctly past -C: {}", result);
    }

    #[test]
    fn test_git_subcmd_basic() {
        let args: Vec<String> = vec!["git".into(), "status".into()];
        assert_eq!(git_subcmd(&args), "status");
    }

    #[test]
    fn test_git_subcmd_with_c_flag() {
        let args: Vec<String> = vec!["git".into(), "-C".into(), "/repo".into(), "log".into()];
        assert_eq!(git_subcmd(&args), "log");
    }

    #[test]
    fn test_git_subcmd_with_c_and_config() {
        let args: Vec<String> = vec!["git".into(), "-C".into(), "/repo".into(), "-c".into(), "k=v".into(), "diff".into()];
        assert_eq!(git_subcmd(&args), "diff");
    }

    #[test]
    fn test_git_subcmd_empty() {
        let args: Vec<String> = vec!["git".into()];
        assert_eq!(git_subcmd(&args), "");
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
        let output = "M  src/main.rs\nA  src/new.rs\n?? untracked.txt\n?? other.txt\n";
        let result = filter_status(output);
        assert!(result.contains("Staged: 2"), "expected Staged: 2, got: {}", result);
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
    fn test_status_caps_overflow() {
        let mut output = String::new();
        for i in 0..20 {
            output.push_str(&format!(" M src/file{}.rs\n", i));
        }
        let result = filter_status(&output);
        assert!(result.contains("[+5 more staged/modified]"), "got: {}", result);
    }

    #[test]
    fn test_diff_hunk_cap() {
        let mut input = "diff --git a/foo.rs b/foo.rs\n--- a/foo.rs\n+++ b/foo.rs\n@@ -1,40 +1,40 @@ fn main() {\n".to_string();
        for i in 0..35 {
            input.push_str(&format!("+    line {};\n", i));
        }
        let result = filter_diff(&input);
        assert!(result.contains("[...truncated...]"), "should truncate at 30 lines, got: {}", result);
    }

    #[test]
    fn test_diff_strips_headers() {
        let output = "diff --git a/foo.rs b/foo.rs\nindex abc..def 100644\n--- a/foo.rs\n+++ b/foo.rs\n@@ -1,3 +1,3 @@ fn main() {\n-    old();\n+    new();\n";
        let result = filter_diff(output);
        assert!(!result.contains("index abc"), "index line should be stripped");
        assert!(!result.contains("--- a/"), "--- line should be stripped");
        assert!(!result.contains("+++ b/"), "+++ line should be stripped");
        assert!(result.contains("-    old();"));
        assert!(result.contains("+    new();"));
    }

    #[test]
    fn test_diff_hunk_context_extracted() {
        let output = "diff --git a/foo.rs b/foo.rs\n--- a/foo.rs\n+++ b/foo.rs\n@@ -10,5 +10,5 @@ fn main() {\n-    old();\n+    new();\n";
        let result = filter_diff(output);
        assert!(result.contains("@@ fn main()"), "hunk context should be kept, got: {}", result);
    }

    #[test]
    fn test_diff_per_file_tally_with_dissimilar_lines() {
        // "old" → "new" has Jaccard=0 (no shared tokens), so both count independently.
        // "extra" is unpaired addition. Total: +2 -1.
        let output = "diff --git a/foo.rs b/foo.rs\n--- a/foo.rs\n+++ b/foo.rs\n@@ -1,3 +1,4 @@\n-old\n+new\n+extra\n context\n";
        let result = filter_diff(output);
        assert!(result.contains("foo.rs"), "filename should appear, got: {}", result);
        assert!(result.contains("+new"), "added line should appear, got: {}", result);
        assert!(result.contains("[+2 -1]"), "tally should appear in header, got: {}", result);
    }

    #[test]
    fn test_diff_jaccard_detects_modification() {
        // Lines that share most tokens should be counted as ~modified
        let output = "diff --git a/foo.rs b/foo.rs\n--- a/foo.rs\n+++ b/foo.rs\n@@ -1,3 +1,3 @@\n-    let result = compute_value(input, config);\n+    let result = compute_value(input, new_config);\n context\n";
        let result = filter_diff(output);
        // The lines share most tokens → Jaccard >= 0.5 → ~1 modified
        assert!(result.contains("~1"), "similar lines should be ~modified, got: {}", result);
        assert!(!result.contains("+1") || !result.contains("-1"),
            "should not have separate +/- for a modification, got: {}", result);
    }

    #[test]
    fn test_diff_jaccard_mixed_block() {
        // 2 similar edits + 1 pure addition
        let output = "diff --git a/foo.rs b/foo.rs\n--- a/foo.rs\n+++ b/foo.rs\n@@ -1,5 +1,6 @@\n-    let x = foo(a, b, c);\n-    let y = bar(a, b, c);\n+    let x = foo(a, b, d);\n+    let y = bar(a, b, d);\n+    let z = baz();\n context\n";
        let result = filter_diff(output);
        // x and y lines are similar edits → ~2, z is a pure add → +1
        assert!(result.contains("~2"), "two similar edits should be ~2, got: {}", result);
        assert!(result.contains("+1"), "one pure addition should be +1, got: {}", result);
    }

    #[test]
    fn test_push_up_to_date() {
        let output = "Everything up-to-date\n";
        assert_eq!(filter_push_pull(output), "ok (up to date)");
    }

    #[test]
    fn test_push_success_one_liner() {
        let output = "remote: Counting objects: 3\nremote: Compressing objects: 100%\n   abc1234..def5678  main -> origin/main\n";
        let result = filter_push_pull(output);
        assert_eq!(result, "ok abc1234..def5678  main -> origin/main");
    }

    #[test]
    fn test_push_error_kept() {
        let output = "Everything up-to-date\nerror: failed to push some refs\n";
        let result = filter_push_pull(output);
        assert_ne!(result, "ok (up to date)");
        assert!(result.contains("error:"));
        // context lines within 2 of the error should also be kept
        assert!(result.contains("Everything up-to-date"));
    }

    #[test]
    fn test_push_error_includes_context_lines() {
        let output = "remote: some preamble\nremote: branch protection rule\nerror: failed to push some refs\nremote: see https://example.com for info\nremote: trailing noise\n";
        let result = filter_push_pull(output);
        assert!(result.contains("error:"), "error line kept");
        // The 2 lines before the error should be included
        assert!(result.contains("branch protection rule"), "context before error kept");
        // The 2 lines after the error should be included
        assert!(result.contains("see https://example.com"), "context after error kept");
    }

    #[test]
    fn test_log_strips_trailers() {
        let output = "abc1234 fix: real commit\ndef5678 Signed-off-by: Bot <bot@ci.com>\n5678abc Co-authored-by: Alice <a@b.com>\n";
        let result = filter_log(output);
        assert!(result.contains("fix: real commit"), "real commit should remain");
        assert!(!result.contains("Signed-off-by"), "trailer commits should be stripped");
        assert!(!result.contains("Co-authored-by"), "trailer commits should be stripped");
    }

    #[test]
    fn test_log_caps_at_configured_limit() {
        let cap = log_line_cap();
        let input_count = cap + 15;
        let mut lines: Vec<String> = Vec::new();
        for i in 0..input_count {
            lines.push(format!("abc{:04} commit message {}", i, i));
        }
        let output = lines.join("\n");
        let result = filter_log(&output);
        let result_lines: Vec<&str> = result.lines().collect();
        assert_eq!(result_lines.len(), cap + 1, "expected {} lines, got: {}", cap + 1, result_lines.len());
        assert!(result_lines.last().unwrap().contains(&format!("{} total", input_count)),
            "should show overflow: {}", result_lines.last().unwrap());
    }

    #[test]
    fn test_commit_format() {
        let output = "[main abc1234] Add feature\n 2 files changed, 10 insertions(+), 3 deletions(-)\n";
        let result = filter_commit(output);
        assert!(result.starts_with("ok — [main abc1234]"), "got: {}", result);
        assert!(result.contains("2 files changed"), "got: {}", result);
    }

    // ─── clone ───────────────────────────────────────────────────────────────

    #[test]
    fn test_clone_strips_progress_keeps_dir() {
        let output = "Cloning into 'my-repo'...\nremote: Enumerating objects: 100, done.\nremote: Counting objects: 100%\nReceiving objects: 100% (100/100), done.\nResolving deltas: 100%\n";
        let result = filter_clone(output);
        assert_eq!(result, "ok — cloned 'my-repo'");
    }

    #[test]
    fn test_clone_error_kept() {
        let output = "Cloning into 'repo'...\nfatal: repository 'https://example.com/repo.git' not found\n";
        let result = filter_clone(output);
        assert!(result.contains("fatal:"), "got: {}", result);
    }

    // ─── merge ───────────────────────────────────────────────────────────────

    #[test]
    fn test_merge_fast_forward_compressed() {
        let output = "Updating abc..def\nFast-forward\n src/main.rs | 2 ++\n 1 file changed, 2 insertions(+)\n";
        let result = filter_merge(output);
        assert_eq!(result, "ok (fast-forward)");
    }

    #[test]
    fn test_merge_conflict_kept() {
        let output = "Auto-merging src/main.rs\nCONFLICT (content): Merge conflict in src/main.rs\nAutomatic merge failed; fix conflicts and then commit.\n";
        let result = filter_merge(output);
        assert!(result.contains("CONFLICT"), "got: {}", result);
        assert!(result.contains("Automatic merge failed"), "got: {}", result);
    }

    // ─── rebase ──────────────────────────────────────────────────────────────

    #[test]
    fn test_rebase_success_compressed() {
        let output = "Successfully rebased and updated refs/heads/feature.\n";
        let result = filter_rebase(output);
        assert!(result.starts_with("ok —"), "got: {}", result);
        assert!(result.contains("rebased"), "got: {}", result);
    }

    #[test]
    fn test_rebase_conflict_kept() {
        let output = "CONFLICT (content): Merge conflict in src/lib.rs\ncould not apply abc1234... some commit\n";
        let result = filter_rebase(output);
        assert!(result.contains("CONFLICT"), "got: {}", result);
    }

    // ─── checkout / switch ───────────────────────────────────────────────────

    #[test]
    fn test_checkout_existing_branch() {
        let output = "Switched to branch 'main'\nYour branch is up to date with 'origin/main'.\n";
        let result = filter_checkout(output);
        assert_eq!(result, "ok — switched to 'main'");
    }

    #[test]
    fn test_checkout_new_branch() {
        let output = "Switched to a new branch 'feature/my-feature'\n";
        let result = filter_checkout(output);
        assert_eq!(result, "ok — new branch 'feature/my-feature'");
    }

    #[test]
    fn test_checkout_error_kept() {
        let output = "error: Your local changes to the following files would be overwritten by checkout:\n\tsrc/main.rs\nPlease commit your changes or stash them before you switch branches.\n";
        let result = filter_checkout(output);
        assert!(result.contains("error:"), "got: {}", result);
    }

    #[test]
    fn test_switch_compressed() {
        let output = "Switched to branch 'develop'\n";
        let result = filter_checkout(output);
        assert_eq!(result, "ok — switched to 'develop'");
    }

    // ─── jaccard similarity ─────────────────────────────────────────────────

    #[test]
    fn test_jaccard_identical() {
        assert_eq!(jaccard_similarity("let x = foo();", "let x = foo();"), 1.0);
    }

    #[test]
    fn test_jaccard_completely_different() {
        assert_eq!(jaccard_similarity("alpha beta gamma", "delta epsilon zeta"), 0.0);
    }

    #[test]
    fn test_jaccard_partial_overlap() {
        // "let x = foo(a, b, c);" vs "let x = foo(a, b, d);"
        // Tokens: {let, x, =, foo(a,, b,, c);} vs {let, x, =, foo(a,, b,, d);}
        // Shared: {let, x, =, foo(a,, b,} = 5, Union = 7
        let sim = jaccard_similarity("let x = foo(a, b, c);", "let x = foo(a, b, d);");
        assert!(sim > 0.5, "similar lines should have Jaccard > 0.5, got: {}", sim);
    }

    #[test]
    fn test_jaccard_empty_lines() {
        assert_eq!(jaccard_similarity("", ""), 1.0);
        assert_eq!(jaccard_similarity("   ", "   "), 1.0);
    }

    // ─── diff line truncation ───────────────────────────────────────────────

    #[test]
    fn test_diff_line_truncation() {
        let long_line = format!("+{}", "x".repeat(200));
        let output = format!(
            "diff --git a/foo.rs b/foo.rs\n--- a/foo.rs\n+++ b/foo.rs\n@@ -1,1 +1,1 @@\n{}\n",
            long_line
        );
        let result = filter_diff(&output);
        // The emitted + line should be truncated to DIFF_LINE_CHAR_CAP
        for line in result.lines() {
            if line.starts_with('+') {
                assert!(line.chars().count() <= DIFF_LINE_CHAR_CAP,
                    "line should be capped at {} chars, got {}: {}",
                    DIFF_LINE_CHAR_CAP, line.chars().count(), line);
                assert!(line.ends_with('…'), "truncated line should end with …, got: {}", line);
            }
        }
    }

    #[test]
    fn test_diff_short_line_not_truncated() {
        let output = "diff --git a/foo.rs b/foo.rs\n--- a/foo.rs\n+++ b/foo.rs\n@@ -1,1 +1,1 @@\n+    short line\n";
        let result = filter_diff(output);
        assert!(result.contains("+    short line"), "short lines should not be truncated, got: {}", result);
        assert!(!result.contains('…'), "should have no ellipsis, got: {}", result);
    }

    // ─── classify_changes ───────────────────────────────────────────────────

    #[test]
    fn test_classify_all_modifications() {
        let removed = vec!["-    let x = compute(a, b, c);", "-    let y = process(d, e, f);"];
        let added   = vec!["+    let x = compute(a, b, z);", "+    let y = process(d, e, z);"];
        let (a, r, m) = classify_changes(&removed, &added);
        assert_eq!(m, 2, "both pairs should be modifications");
        assert_eq!(a, 0);
        assert_eq!(r, 0);
    }

    #[test]
    fn test_classify_pure_additions() {
        let removed: Vec<&str> = vec![];
        let added = vec!["+    new_line_1();", "+    new_line_2();"];
        let (a, r, m) = classify_changes(&removed, &added);
        assert_eq!(a, 2);
        assert_eq!(r, 0);
        assert_eq!(m, 0);
    }

    #[test]
    fn test_classify_pure_removals() {
        let removed = vec!["-    old_line_1();", "-    old_line_2();"];
        let added: Vec<&str> = vec![];
        let (a, r, m) = classify_changes(&removed, &added);
        assert_eq!(a, 0);
        assert_eq!(r, 2);
        assert_eq!(m, 0);
    }

    #[test]
    fn test_classify_mixed() {
        // 1 similar pair (modification) + 1 extra addition
        let removed = vec!["-    let x = foo(a, b, c);"];
        let added   = vec!["+    let x = foo(a, b, d);", "+    let z = brand_new();"];
        let (a, r, m) = classify_changes(&removed, &added);
        assert_eq!(m, 1, "similar pair should be modification");
        assert_eq!(a, 1, "extra added line should be pure addition");
        assert_eq!(r, 0);
    }
}
