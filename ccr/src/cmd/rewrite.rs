use anyhow::Result;
use once_cell::sync::Lazy;
use regex::Regex;
use std::borrow::Cow;

/// Matches a bash line-continuation: backslash + optional horizontal whitespace
/// before the `\n` (or `\r\n`) + optional horizontal whitespace after. This is
/// what bash itself collapses to a single space before executing, so the hook
/// rewriter must do the same or multi-line commands bypass the matcher entirely.
static LINE_CONTINUATION_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)[ \t\x0B\x0C]*\\\r?\n[ \t\x0B\x0C]*").unwrap());

/// Replace every bash line continuation with a single space, mirroring bash.
/// Returns a borrowed `&str` when the input has no continuations (zero allocation).
fn collapse_line_continuations(s: &str) -> Cow<'_, str> {
    LINE_CONTINUATION_RE.replace_all(s, " ")
}

/// Shell prefix builtins that modify *how* the shell runs a command without
/// changing *which* command runs. Strip before routing; re-prepend after rewrite.
const SHELL_PREFIX_BUILTINS: &[&str] = &["noglob", "command", "builtin", "exec", "nocorrect"];

/// Maximum number of transparent-prefix stripping passes to prevent infinite
/// recursion when a user configures a prefix that keeps matching itself.
const MAX_PREFIX_DEPTH: usize = 10;

/// Sort prefixes longest-first (so `docker exec mycontainer` wins over `docker`)
/// and dedup. Returns a new vec; input is unchanged.
fn normalize_transparent_prefixes(prefixes: &[String]) -> Vec<String> {
    let mut out: Vec<String> = prefixes
        .iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    out.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    out.dedup();
    out
}

/// Strip `prefix` from `cmd` with a strict word-boundary check.
/// Returns the rest of the command (trimmed) on match, or `None`.
fn strip_word_prefix<'a>(cmd: &'a str, prefix: &str) -> Option<&'a str> {
    if cmd == prefix {
        Some("")
    } else if cmd.len() > prefix.len()
        && cmd.starts_with(prefix)
        && cmd.as_bytes()[prefix.len()] == b' '
    {
        Some(cmd[prefix.len() + 1..].trim_start())
    } else {
        None
    }
}

/// Load user-configured transparent prefixes from the global config.
fn load_transparent_prefixes() -> Vec<String> {
    crate::config_loader::load_config()
        .map(|c| c.hooks.transparent_prefixes)
        .unwrap_or_default()
}

/// Returns the full path to the running panda binary so rewritten commands
/// work in non-interactive shells where `~/.cargo/bin` may not be in PATH.
/// Falls back to `"panda"` if the path cannot be determined.
fn panda_bin() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_else(|| "panda".to_string())
}

/// Rewrite a command string for PreToolUse injection.
/// Prints the rewritten command and exits 0, or exits 1 if no rewrite is needed.
pub fn run(command: String) -> Result<()> {
    let rewritten = rewrite_command(&command);
    match rewritten {
        Some(r) => {
            print!("{}", r);
            Ok(())
        }
        None => {
            // No rewrite — exit 1 so the hook passes through silently
            std::process::exit(1);
        }
    }
}

/// Rewrite a full command string. Returns `Some(rewritten)` if rewrite is needed,
/// or `None` if no handler matches or already wrapped.
///
/// Normalizes bash line continuations (`\<NL>`) and strips transparent wrapper
/// prefixes (built-ins + user-configured `[hooks].transparent_prefixes`) before
/// routing, re-prepending them after the rewrite.
pub fn rewrite_command(command: &str) -> Option<String> {
    let normalized = collapse_line_continuations(command);
    let command = normalized.as_ref();

    let raw_prefixes = load_transparent_prefixes();
    let prefixes = normalize_transparent_prefixes(&raw_prefixes);

    rewrite_command_impl(command, &prefixes)
}

/// Inner implementation; accepts already-normalized command and prefixes.
/// Separated so tests can inject prefixes without touching the config file.
fn rewrite_command_impl(command: &str, prefixes: &[String]) -> Option<String> {
    // Handle compound commands: &&, ||, ;
    if let Some(result) = rewrite_compound(command, " && ", prefixes) {
        return Some(result);
    }
    if let Some(result) = rewrite_compound(command, " || ", prefixes) {
        return Some(result);
    }
    if let Some(result) = rewrite_compound(command, "; ", prefixes) {
        return Some(result);
    }

    // Single command
    rewrite_single_inner(command, prefixes, 0)
}

/// Returns the byte offset where the actual command starts, after any leading
/// `KEY=VALUE` environment-variable prefix tokens.
///
/// Quote-aware: `KEY="val with spaces" cargo build` correctly identifies `cargo`
/// as the command start despite the space inside the quoted value.
///
/// Example: `"RUST_LOG=debug cargo build"` → 15 (offset of `cargo`)
fn env_prefix_end(s: &str) -> usize {
    let mut pos = 0;
    let bytes = s.as_bytes();
    loop {
        // skip whitespace
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if pos >= bytes.len() {
            return pos;
        }
        let tok_start = pos;
        pos = scan_token(s, tok_start);
        let token = &s[tok_start..pos];
        if !is_env_var_assignment(token) {
            return tok_start;
        }
        // was KEY=VALUE — keep scanning
    }
}

/// Scan one shell token starting at `start`, consuming quoted strings as a unit.
/// Returns the byte offset of the first whitespace (or end of string) after the token.
///
/// Handles single-quoted strings (no escape processing) and double-quoted strings
/// (backslash escapes), so a token like `KEY="val with spaces"` is scanned as one unit.
fn scan_token(s: &str, start: usize) -> usize {
    let bytes = s.as_bytes();
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b if b.is_ascii_whitespace() => break,
            b'\'' => {
                // Single-quoted: scan until closing ', no escape processing
                i += 1;
                while i < bytes.len() && bytes[i] != b'\'' {
                    i += 1;
                }
                if i < bytes.len() { i += 1; } // consume closing '
            }
            b'"' => {
                // Double-quoted: scan until closing ", honouring backslash escapes
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i += 2; // skip escaped character
                    } else if bytes[i] == b'"' {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
            }
            _ => i += 1,
        }
    }
    i
}

/// Returns true if `token` looks like a shell environment-variable assignment
/// (`KEY=VALUE` where KEY is `[A-Za-z_][A-Za-z0-9_]*`).
fn is_env_var_assignment(token: &str) -> bool {
    if let Some((key, _)) = token.split_once('=') {
        !key.is_empty()
            && key.chars().next().map(|c| c.is_ascii_alphabetic() || c == '_').unwrap_or(false)
            && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    } else {
        false
    }
}

/// Returns true if `command` contains a stdout redirect (`>`, `>>`) or pipe
/// (`|`) outside of single or double quotes.
///
/// Simple heuristic — not a full shell parser. Commands whose stdout is
/// diverted to a file or another process must not be wrapped with `ccr run`,
/// because CCR's dedup/delta annotations would replace the real content.
fn has_stdout_diversion(command: &str) -> bool {
    let mut in_single = false;
    let mut in_double = false;
    let bytes = command.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' if !in_double => { in_single = !in_single; }
            b'"'  if !in_single => { in_double = !in_double; }
            b'>'  if !in_single && !in_double => {
                // Exclude '->' and '=>' (not a redirect)
                let prev = if i > 0 { bytes[i - 1] } else { b' ' };
                if prev != b'-' && prev != b'=' {
                    return true;
                }
            }
            b'|'  if !in_single && !in_double => {
                // '||' is logical OR, not a pipe — skip both characters
                if i + 1 < bytes.len() && bytes[i + 1] == b'|' {
                    i += 1; // skip the second '|'
                } else {
                    return true;
                }
            }
            _ => {}
        }
        i += 1;
    }
    false
}

/// Rewrite `head [-N] file` and `tail [-N] file` to `ccr run cat file`,
/// routing through ReadHandler for code-aware filtering instead of raw truncation.
///
/// Handles: `head file`, `head -N file`, `head -n N file`, `head --lines=N file`,
/// and the same variants for `tail`. Skips byte-mode (`-c`), follow-mode (`-f`),
/// multi-file invocations, and stdin (`-`).
fn rewrite_head_tail(cmd: &str) -> Option<String> {
    let args: Vec<&str> = cmd.split_whitespace().collect();
    let binary = *args.first()?;
    if binary != "head" && binary != "tail" {
        return None;
    }

    let mut file: Option<&str> = None;
    let mut i = 1;
    while i < args.len() {
        let a = args[i];
        if a.starts_with('-') {
            match a {
                // Byte mode or follow mode — unsupported, bail
                "-c" | "--bytes" | "-f" | "-F" | "--follow" => return None,
                // -n N or --lines N — skip the value argument too
                "-n" | "--lines" => { i += 2; continue; }
                _ => {
                    if a.starts_with("--lines=") { i += 1; continue; }
                    // -N where N is all digits (e.g. -20, -100)
                    let digits = a.trim_start_matches('-');
                    if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                        i += 1; continue;
                    }
                    // Any other flag — bail
                    return None;
                }
            }
        }
        // File argument
        if file.is_some() { return None; } // multiple files — skip
        file = Some(a);
        i += 1;
    }

    let file = file?; // no file argument (reading stdin) — skip
    if file == "-" { return None; }

    Some(format!("{} run cat {}", panda_bin(), file))
}

/// Rewrite a single (non-compound) command, trying transparent prefixes first.
/// `depth` guards against infinite recursion when prefixes nest.
fn rewrite_single_inner(command: &str, prefixes: &[String], depth: usize) -> Option<String> {
    if depth >= MAX_PREFIX_DEPTH {
        return None;
    }

    let trimmed = command.trim();

    // Don't double-wrap
    if trimmed.starts_with("panda run ") || trimmed == "panda run" {
        return None;
    }

    // Never wrap commands that divert stdout (redirect or pipe).
    // CCR's dedup/delta annotations would replace real content.
    if has_stdout_diversion(trimmed) {
        return None;
    }

    // ── Shell built-in transparent prefixes ──────────────────────────────────
    // noglob, command, builtin, exec, nocorrect — strip, rewrite inner, re-prepend.
    for &builtin in SHELL_PREFIX_BUILTINS {
        if let Some(rest) = strip_word_prefix(trimmed, builtin) {
            if rest.is_empty() {
                return None;
            }
            return rewrite_single_inner(rest, prefixes, depth + 1)
                .map(|r| format!("{} {}", builtin, r));
        }
    }

    // ── User-configured transparent prefixes ─────────────────────────────────
    // e.g. "direnv exec .", "docker exec mycontainer"
    for prefix in prefixes {
        if let Some(rest) = strip_word_prefix(trimmed, prefix) {
            if rest.is_empty() {
                return None;
            }
            return rewrite_single_inner(rest, prefixes, depth + 1)
                .map(|r| format!("{} {}", prefix, r));
        }
    }

    // ── Route the actual command ─────────────────────────────────────────────

    // Strip any leading KEY=VALUE env-variable prefix tokens so we can match
    // the actual command name (e.g. `RUST_LOG=debug cargo build` → `cargo`).
    let cmd_start = env_prefix_end(trimmed);
    let env_part = &trimmed[..cmd_start]; // e.g. "RUST_LOG=debug " or ""
    let cmd_part = trimmed[cmd_start..].trim_start();

    // head/tail file → ccr run cat file (ReadHandler applies code-aware filtering)
    if let Some(r) = rewrite_head_tail(cmd_part) {
        return Some(format!("{}{}", env_part, r));
    }

    // Extract argv[0]
    let first = cmd_part.split_whitespace().next()?;

    let handler = crate::handlers::get_handler(first)?;

    // Build the flag-injected arg list via the handler (no env prefix in args)
    let args: Vec<String> = cmd_part.split_whitespace().map(String::from).collect();
    let rewritten_args = handler.rewrite_args(&args);

    // Preserve env prefix before `panda run` so the shell sets those vars for the process.
    Some(format!("{}{} run {}", env_part, panda_bin(), rewritten_args.join(" ")))
}

/// Try to split a compound command on `operator` and rewrite each part.
/// Returns `Some(rewritten)` only if at least one part was rewritten.
fn rewrite_compound(command: &str, operator: &str, prefixes: &[String]) -> Option<String> {
    if !command.contains(operator) {
        return None;
    }

    let parts: Vec<&str> = command.split(operator).collect();
    if parts.len() < 2 {
        return None;
    }

    let mut any_rewritten = false;
    let rewritten: Vec<String> = parts
        .iter()
        .map(|part| {
            if let Some(r) = rewrite_single_inner(part.trim(), prefixes, 0) {
                any_rewritten = true;
                r
            } else {
                part.trim().to_string()
            }
        })
        .collect();

    if any_rewritten {
        Some(rewritten.join(operator))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_command_rewritten() {
        let result = rewrite_command("git status");
        // git status gets --porcelain injected via rewrite_args
        let r = result.expect("git status should be rewritten");
        assert!(r.contains("run git status --porcelain"), "got: {}", r);
    }

    #[test]
    fn flag_injection_for_cargo_build() {
        let result = rewrite_command("cargo build");
        // cargo build gets --message-format json injected
        let r = result.expect("cargo build should be rewritten");
        assert!(r.contains("run cargo build"), "should be wrapped: {}", r);
        assert!(r.contains("--message-format"), "should inject --message-format: {}", r);
        assert!(r.contains("json"), "should inject json format: {}", r);
    }

    #[test]
    fn no_double_flag_injection() {
        // If --message-format already present, it should not be added again
        let result = rewrite_command("cargo build --message-format human");
        let r = result.expect("should be rewritten");
        let count = r.matches("--message-format").count();
        assert_eq!(count, 1, "flag should appear exactly once: {}", r);
    }

    #[test]
    fn unknown_command_not_rewritten() {
        let result = rewrite_command("some-unknown-tool --flag");
        assert_eq!(result, None);
    }

    #[test]
    fn no_double_wrap() {
        // Commands already containing "panda run" must not be wrapped again.
        // Use a literal prefix since the binary path varies by environment.
        let result = rewrite_command("panda run git status");
        assert_eq!(result, None);
    }

    #[test]
    fn compound_and() {
        let result = rewrite_command("cargo build && git push");
        let r = result.expect("should be rewritten");
        assert!(r.contains("run cargo build"), "cargo part: {}", r);
        assert!(r.contains("run git push"), "git part: {}", r);
        assert!(r.contains(" && "), "should preserve && operator: {}", r);
    }

    #[test]
    fn compound_mixed() {
        // Only known commands get wrapped; git status gets --porcelain injected
        let result = rewrite_command("some-tool && git status");
        let r = result.expect("should rewrite the git part");
        assert!(r.starts_with("some-tool &&"), "should preserve unknown tool: {}", r);
        assert!(r.contains("run git status --porcelain"), "should wrap git: {}", r);
    }

    #[test]
    fn compound_no_known() {
        // No known commands → no rewrite
        let result = rewrite_command("tool-a && tool-b");
        assert_eq!(result, None);
    }

    #[test]
    fn redirect_bare() {
        assert!(has_stdout_diversion("git show HEAD:src/main.rs > main.rs"));
    }

    #[test]
    fn redirect_append() {
        assert!(has_stdout_diversion("cargo build >> build.log"));
    }

    #[test]
    fn redirect_inside_single_quotes_not_detected() {
        // > inside quotes is not a redirect
        assert!(!has_stdout_diversion("echo 'a > b'"));
    }

    #[test]
    fn redirect_inside_double_quotes_not_detected() {
        assert!(!has_stdout_diversion("echo \"a > b\""));
    }

    #[test]
    fn arrow_operators_not_redirect() {
        // -> and => in code snippets / descriptions must not trigger
        assert!(!has_stdout_diversion("git log --format='%H -> %s'"));
        assert!(!has_stdout_diversion("some-tool => output"));
    }

    #[test]
    fn pipe_detected() {
        assert!(has_stdout_diversion("git log | head -5"));
    }

    #[test]
    fn pipe_inside_quotes_not_detected() {
        assert!(!has_stdout_diversion("echo 'a | b'"));
        assert!(!has_stdout_diversion("echo \"a | b\""));
    }

    #[test]
    fn logical_or_not_detected_as_pipe() {
        assert!(!has_stdout_diversion("test -f foo || echo missing"));
    }

    #[test]
    fn pipe_with_redirect_detected() {
        assert!(has_stdout_diversion("git show HEAD:file | head -1"));
    }

    #[test]
    fn piped_command_not_wrapped() {
        let result = rewrite_command("git show HEAD:file | head -5");
        assert_eq!(result, None, "should not wrap a piped command");
    }

    #[test]
    fn subshell_pipe_detected_as_false_positive() {
        // Pipes inside $() are detected — accepted trade-off since we are
        // not a full shell parser. Prevents wrapping, which is the safe default.
        assert!(has_stdout_diversion("echo $(git log | head -1)"));
    }

    #[test]
    fn pipe_at_start_of_string() {
        assert!(has_stdout_diversion("| cat"));
    }

    #[test]
    fn compound_with_pipe_only_wraps_non_piped_part() {
        // rewrite_compound splits on && then has_stdout_diversion guards each part.
        // The piped part must NOT be wrapped; the non-piped part should be.
        let result = rewrite_command("cargo build && git log | head -5");
        assert!(result.is_some(), "compound should still rewrite the non-piped part");
        let r = result.unwrap();
        assert!(r.contains("run cargo build"), "cargo build should be wrapped");
        assert!(!r.contains("run git log"), "piped git log must not be wrapped");
        assert!(r.contains("git log | head -5"), "piped part should pass through unchanged");
    }

    #[test]
    fn git_show_redirect_not_wrapped() {
        // git show with redirect must not be wrapped — would corrupt the output file
        let result = rewrite_command("git show origin/main:src/lib.rs > /tmp/lib.rs");
        assert_eq!(result, None, "should not wrap a redirected command");
    }

    #[test]
    fn git_show_no_redirect_still_wrapped() {
        // git show without redirect should still be wrapped normally
        let result = rewrite_command("git show HEAD");
        assert!(result.is_some(), "should wrap git show without redirect");
        assert!(result.unwrap().contains("run git show"));
    }

    // ── env prefix tests ──────────────────────────────────────────────────────

    #[test]
    fn env_prefix_single_var() {
        let result = rewrite_command("RUST_LOG=debug cargo build");
        let r = result.expect("should rewrite despite env prefix");
        assert!(r.starts_with("RUST_LOG=debug "), "should preserve env prefix: {}", r);
        assert!(r.contains("run cargo build"), "should wrap cargo: {}", r);
        assert!(r.contains("--message-format"), "should still inject --message-format: {}", r);
    }

    #[test]
    fn env_prefix_multiple_vars() {
        let result = rewrite_command("CI=1 NODE_ENV=production npm install");
        let r = result.expect("should rewrite despite multiple env prefixes");
        assert!(r.starts_with("CI=1 NODE_ENV=production "), "should preserve env prefix: {}", r);
        assert!(r.contains("run npm"), "should wrap npm: {}", r);
    }

    #[test]
    fn env_prefix_no_handler_still_none() {
        let result = rewrite_command("RUST_LOG=debug unknown-tool --flag");
        assert_eq!(result, None, "no handler → no rewrite even with env prefix");
    }

    #[test]
    fn is_env_var_assignment_valid() {
        assert!(is_env_var_assignment("RUST_LOG=debug"));
        assert!(is_env_var_assignment("CI=1"));
        assert!(is_env_var_assignment("_VAR=value"));
        assert!(is_env_var_assignment("KEY="));        // empty value is valid
    }

    #[test]
    fn is_env_var_assignment_invalid() {
        assert!(!is_env_var_assignment("cargo"));      // no '='
        assert!(!is_env_var_assignment("--flag=val")); // starts with '-'
        assert!(!is_env_var_assignment("1KEY=val"));   // starts with digit
        assert!(!is_env_var_assignment("=value"));     // empty key
    }

    #[test]
    fn env_prefix_compound_command() {
        let result = rewrite_command("CI=1 cargo build && git status");
        let r = result.expect("should rewrite compound with env prefix");
        assert!(r.starts_with("CI=1 "), "should preserve env prefix: {}", r);
        assert!(r.contains("run cargo build"), "cargo part: {}", r);
        assert!(r.contains("run git status"), "git part: {}", r);
    }

    // ── quoted env prefix tests ───────────────────────────────────────────────

    #[test]
    fn env_prefix_quoted_double_value() {
        let result = rewrite_command("KEY=\"val with spaces\" cargo build");
        let r = result.expect("should rewrite despite quoted env value");
        assert!(r.contains("run cargo build"), "got: {}", r);
        assert!(r.contains("--message-format"), "should inject flag: {}", r);
    }

    #[test]
    fn env_prefix_quoted_single_value() {
        let result = rewrite_command("NODE_ENV='production mode' npm install");
        let r = result.expect("should rewrite despite single-quoted env value");
        assert!(r.contains("run npm"), "got: {}", r);
    }

    #[test]
    fn scan_token_plain() {
        assert_eq!(scan_token("cargo build", 0), 5); // "cargo"
    }

    #[test]
    fn scan_token_double_quoted() {
        // KEY="val with spaces" → token ends after closing "
        let s = r#"KEY="val with spaces" cargo"#;
        let end = scan_token(s, 0);
        assert_eq!(&s[..end], r#"KEY="val with spaces""#);
    }

    #[test]
    fn scan_token_single_quoted() {
        let s = "KEY='val with spaces' cargo";
        let end = scan_token(s, 0);
        assert_eq!(&s[..end], "KEY='val with spaces'");
    }

    #[test]
    fn scan_token_escaped_in_double_quotes() {
        let s = r#"KEY="val\"quoted" cargo"#;
        let end = scan_token(s, 0);
        assert_eq!(&s[..end], r#"KEY="val\"quoted""#);
    }

    // ── head / tail rewrite tests ─────────────────────────────────────────────

    #[test]
    fn head_plain_file() {
        let r = rewrite_command("head src/main.rs").expect("should rewrite");
        assert!(r.contains("run cat src/main.rs"), "got: {}", r);
    }

    #[test]
    fn head_numeric_flag() {
        let r = rewrite_command("head -20 src/main.rs").expect("should rewrite");
        assert!(r.contains("run cat src/main.rs"), "got: {}", r);
    }

    #[test]
    fn head_n_flag_with_space() {
        let r = rewrite_command("head -n 50 src/lib.rs").expect("should rewrite");
        assert!(r.contains("run cat src/lib.rs"), "got: {}", r);
    }

    #[test]
    fn head_lines_long_flag() {
        let r = rewrite_command("head --lines=30 README.md").expect("should rewrite");
        assert!(r.contains("run cat README.md"), "got: {}", r);
    }

    #[test]
    fn tail_numeric_flag() {
        let r = rewrite_command("tail -20 src/main.rs").expect("should rewrite");
        assert!(r.contains("run cat src/main.rs"), "got: {}", r);
    }

    #[test]
    fn tail_n_flag_with_space() {
        let r = rewrite_command("tail -n 10 src/lib.rs").expect("should rewrite");
        assert!(r.contains("run cat src/lib.rs"), "got: {}", r);
    }

    #[test]
    fn head_byte_mode_skipped() {
        assert_eq!(rewrite_command("head -c 100 src/main.rs"), None);
    }

    #[test]
    fn tail_follow_mode_skipped() {
        assert_eq!(rewrite_command("tail -f /var/log/app.log"), None);
    }

    #[test]
    fn head_multiple_files_skipped() {
        assert_eq!(rewrite_command("head -20 a.rs b.rs"), None);
    }

    #[test]
    fn head_no_file_skipped() {
        // head with no file reads stdin — don't rewrite
        assert_eq!(rewrite_command("head -20"), None);
    }

    #[test]
    fn head_stdin_dash_skipped() {
        assert_eq!(rewrite_command("head -20 -"), None);
    }

    #[test]
    fn head_in_compound_with_git() {
        let result = rewrite_command("head -50 src/main.rs && git status");
        let r = result.expect("compound should rewrite");
        assert!(r.contains("run cat src/main.rs"), "head part: {}", r);
        assert!(r.contains("run git status"), "git part: {}", r);
    }

    // ── line-continuation tests ───────────────────────────────────────────────

    /// Helper: rewrite with explicit transparent prefixes (bypasses config file).
    fn rewrite_with_prefixes(cmd: &str, prefixes: &[&str]) -> Option<String> {
        let normalized = collapse_line_continuations(cmd);
        let p: Vec<String> = prefixes.iter().map(|s| s.to_string()).collect();
        let p = normalize_transparent_prefixes(&p);
        rewrite_command_impl(normalized.as_ref(), &p)
    }

    #[test]
    fn line_continuation_basic() {
        // "git status \<NL>" should collapse to "git status" and be rewritten
        let cmd = "git status \\\n";
        let r = rewrite_command(cmd).expect("should rewrite after line-continuation collapse");
        assert!(r.contains("run git status --porcelain"), "got: {}", r);
    }

    #[test]
    fn line_continuation_mid_command() {
        // "git \<NL>status" should collapse to "git status"
        let cmd = "git \\\nstatus";
        let r = rewrite_command(cmd).expect("should rewrite");
        assert!(r.contains("run git status"), "got: {}", r);
    }

    #[test]
    fn line_continuation_with_spaces_around_break() {
        // "git status   \<NL>   " — horizontal whitespace before/after break collapses to one space
        let cmd = "git status   \\\n   ";
        let r = rewrite_command(cmd).expect("should rewrite after collapsing spaces around break");
        assert!(r.contains("run git status"), "got: {}", r);
    }

    #[test]
    fn line_continuation_crlf() {
        let cmd = "git status \\\r\n";
        let r = rewrite_command(cmd).expect("CRLF line continuation should be collapsed");
        assert!(r.contains("run git status"), "got: {}", r);
    }

    #[test]
    fn line_continuation_compound() {
        // "cargo build \<NL>&& git status" should rewrite both sides
        let cmd = "cargo build \\\n&& git status";
        let r = rewrite_command(cmd).expect("compound with continuation should rewrite");
        assert!(r.contains("run cargo build"), "cargo part: {}", r);
        assert!(r.contains("run git status"), "git part: {}", r);
    }

    #[test]
    fn collapse_line_continuations_no_alloc_on_clean_input() {
        // When there are no line continuations, collapse_line_continuations should
        // return a Borrowed variant (zero allocation fast path).
        let s = "git status";
        let result = collapse_line_continuations(s);
        assert_eq!(result.as_ref(), "git status");
    }

    // ── transparent_prefix tests ─────────────────────────────────────────────

    #[test]
    fn builtin_exec_prefix_stripped() {
        // "exec git status" → strip "exec", rewrite inner "git status"
        let r = rewrite_command("exec git status").expect("should rewrite");
        assert!(r.starts_with("exec "), "should re-prepend exec: {}", r);
        assert!(r.contains("run git status"), "inner cmd should be rewritten: {}", r);
    }

    #[test]
    fn builtin_command_prefix_stripped() {
        let r = rewrite_command("command git status").expect("should rewrite");
        assert!(r.starts_with("command "), "got: {}", r);
        assert!(r.contains("run git status"), "got: {}", r);
    }

    #[test]
    fn builtin_noglob_prefix_stripped() {
        let r = rewrite_command("noglob cargo build").expect("should rewrite");
        assert!(r.starts_with("noglob "), "got: {}", r);
        assert!(r.contains("run cargo build"), "got: {}", r);
    }

    #[test]
    fn user_transparent_prefix_stripped() {
        // "direnv exec . git status" with prefix "direnv exec ." configured
        let r = rewrite_with_prefixes("direnv exec . git status", &["direnv exec ."])
            .expect("should rewrite");
        assert!(r.starts_with("direnv exec . "), "should re-prepend prefix: {}", r);
        assert!(r.contains("run git status"), "inner cmd should be wrapped: {}", r);
    }

    #[test]
    fn user_transparent_prefix_longer_wins() {
        // "docker exec mycontainer git status" — longer prefix should match
        let r = rewrite_with_prefixes(
            "docker exec mycontainer git status",
            &["docker exec mycontainer", "docker"],
        )
        .expect("should rewrite");
        assert!(
            r.starts_with("docker exec mycontainer "),
            "longer prefix should win: {}", r
        );
        assert!(r.contains("run git status"), "got: {}", r);
    }

    #[test]
    fn prefix_only_no_inner_command() {
        // A command that IS just the prefix returns None (no inner cmd to route)
        assert_eq!(
            rewrite_with_prefixes("direnv exec .", &["direnv exec ."]),
            None
        );
    }

    #[test]
    fn prefix_inner_unknown_command() {
        // Prefix + unknown inner command still returns None (no handler)
        assert_eq!(
            rewrite_with_prefixes("exec some-unknown-tool", &[]),
            None
        );
    }

    #[test]
    fn prefix_recursion_bounded() {
        // Deeply nested self-referential prefix must not stack overflow
        let prefixes: Vec<&str> = vec!["wrap"];
        let mut cmd = String::new();
        for _ in 0..(MAX_PREFIX_DEPTH + 2) {
            cmd.push_str("wrap ");
        }
        cmd.push_str("git status");
        // Should return None (depth exceeded) or Some — but must not panic
        let _ = rewrite_with_prefixes(&cmd, &prefixes);
    }

    #[test]
    fn normalize_transparent_prefixes_sorts_longest_first() {
        let input: Vec<String> = vec![
            "docker".to_string(),
            "docker exec mycontainer".to_string(),
            "docker exec".to_string(),
        ];
        let out = normalize_transparent_prefixes(&input);
        assert_eq!(out[0], "docker exec mycontainer", "longest should be first");
        assert_eq!(out[1], "docker exec");
        assert_eq!(out[2], "docker");
    }

    #[test]
    fn normalize_transparent_prefixes_deduplicates() {
        let input: Vec<String> = vec!["foo".to_string(), "foo".to_string(), "bar".to_string()];
        let out = normalize_transparent_prefixes(&input);
        assert_eq!(out.iter().filter(|s| *s == "bar").count(), 1);
        assert_eq!(out.iter().filter(|s| *s == "foo").count(), 1);
    }

    #[test]
    fn strip_word_prefix_exact_match() {
        assert_eq!(strip_word_prefix("exec", "exec"), Some(""));
    }

    #[test]
    fn strip_word_prefix_with_rest() {
        assert_eq!(strip_word_prefix("exec git status", "exec"), Some("git status"));
    }

    #[test]
    fn strip_word_prefix_no_match() {
        assert_eq!(strip_word_prefix("executor git status", "exec"), None);
    }

    #[test]
    fn strip_word_prefix_partial_no_space() {
        // "execution" does not match prefix "exec" (no word boundary)
        assert_eq!(strip_word_prefix("execution", "exec"), None);
    }
}
