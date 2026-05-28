use super::util;
use super::Handler;

pub struct PipHandler;

impl Handler for PipHandler {
    fn rewrite_args(&self, args: &[String]) -> Vec<String> {
        let subcmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
        let is_uv = args.get(0).map(|s| s.as_str()).unwrap_or("") == "uv";
        let mut out = args.to_vec();

        if !is_uv && (subcmd == "install" || subcmd == "add") {
            if !out.iter().any(|a| a == "-q" || a == "--quiet") {
                out.push("-q".to_string());
            }
        }

        // Inject --format=json for pip list/outdated to get structured, parseable output.
        // Reduces raw tabular output to compact name+version pairs.
        if !is_uv && (subcmd == "list" || subcmd == "outdated") {
            if !out.iter().any(|a| a.starts_with("--format")) {
                out.push("--format=json".to_string());
            }
        }

        out
    }

    fn filter(&self, output: &str, args: &[String]) -> String {
        let cmd = args.get(0).map(|s| s.as_str()).unwrap_or("");
        let subcmd = args.get(1).map(|s| s.as_str()).unwrap_or("");

        match subcmd {
            "freeze" => return output.to_string(),
            "list" | "outdated" => return filter_pip_list(output),
            "install" | "add" => {
                if cmd == "uv" {
                    return filter_uv_install(output);
                }
                if cmd == "poetry" || cmd == "pdm" {
                    return filter_poetry_install(output);
                }
                return filter_pip_install(output);
            }
            _ => {}
        }

        // Default: keep only final non-empty line
        output
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or(output)
            .to_string()
    }
}

/// Filter `poetry install` / `pdm install` output.
///
/// Poetry's "nothing to do" messages are different from pip's, so they need
/// their own short-circuit patterns:
///   "No dependencies to install or update."
///   "Package operations: 0 installs, 0 updates, 0 removals"
///
/// When something IS installed, emit a one-liner summary instead of all the
/// "  - Installing <pkg> (<ver>)" lines.
fn filter_poetry_install(output: &str) -> String {
    const POETRY_SATISFIED_RULES: &[util::MatchOutputRule] = &[
        util::MatchOutputRule {
            success_pattern: r"(?i)No dependencies to install or update",
            error_pattern: r"(?i)error|failed|warning",
            ok_message: "ok (up to date)",
        },
        util::MatchOutputRule {
            // "Package operations: 0 installs, 0 updates, 0 removals"
            success_pattern: r"Package operations: 0 installs, 0 updates, 0 removals",
            error_pattern: r"(?i)error|failed",
            ok_message: "ok (up to date)",
        },
    ];
    if let Some(msg) = util::check_match_output(output, POETRY_SATISFIED_RULES) {
        return msg;
    }

    // Count installs/updates from "Package operations: N installs, M updates, K removals"
    // and warnings from any WARNING lines.
    let mut summary: Option<String> = None;
    let mut warnings: Vec<String> = Vec::new();

    for line in output.lines() {
        let t = line.trim();
        if t.starts_with("Package operations:") {
            summary = Some(t.to_string());
        } else if t.to_uppercase().starts_with("WARNING") || t.to_uppercase().starts_with("ERROR") {
            warnings.push(line.to_string());
        }
    }

    let mut out: Vec<String> = warnings;
    if let Some(s) = summary {
        out.push(s);
    } else {
        // Fallback: count "Installing" lines
        let installed = output.lines().filter(|l| l.trim().starts_with("- Installing")).count();
        if installed > 0 {
            out.push(format!("[poetry install complete — {} packages]", installed));
        } else {
            return output.to_string();
        }
    }
    out.join("\n")
}

/// Filter `pip list --format=json` or `pip outdated --format=json` output.
///
/// JSON format: `[{"name":"pkg","version":"1.0"}, ...]`
/// Compresses to: `pkg==1.0, pkg2==2.3, ...` with a cap of 50 packages.
/// Falls back to raw output if JSON parse fails (user overrode --format).
fn filter_pip_list(output: &str) -> String {
    // Try JSON parse
    let trimmed = output.trim();
    if trimmed.starts_with('[') {
        if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(trimmed) {
            let cap = 50usize;
            let total = arr.len();
            let mut pkgs: Vec<String> = arr.iter().take(cap).filter_map(|v| {
                let name = v.get("name")?.as_str()?;
                let ver = v.get("version").and_then(|v| v.as_str()).unwrap_or("?");
                // For outdated, also show latest version
                if let Some(latest) = v.get("latest_version").and_then(|v| v.as_str()) {
                    Some(format!("{}=={} → {}", name, ver, latest))
                } else {
                    Some(format!("{}=={}", name, ver))
                }
            }).collect();

            if total > cap {
                pkgs.push(format!("[+{} more packages]", total - cap));
            }
            pkgs.push(format!("[{} packages total]", total));
            return pkgs.join("\n");
        }
    }

    // Non-JSON fallback: cap long tabular output
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() <= 50 {
        return output.to_string();
    }
    let mut out: Vec<String> = lines[..50].iter().map(|l| l.to_string()).collect();
    out.push(format!("[+{} more packages]", lines.len() - 50));
    out.join("\n")
}

fn filter_pip_install(output: &str) -> String {
    const PIP_SATISFIED_RULES: &[util::MatchOutputRule] = &[util::MatchOutputRule {
        success_pattern: r"(?i)Requirement already satisfied|already up-to-date|already installed",
        error_pattern: r"(?i)error|ERROR|Failed|failed",
        ok_message: "ok (already satisfied)",
    }];
    if let Some(msg) = util::check_match_output(output, PIP_SATISFIED_RULES) {
        return msg;
    }

    let mut warnings: Vec<String> = Vec::new();
    let mut installed = 0usize;

    for line in output.lines() {
        let t = line.trim();
        if t.starts_with("Successfully installed") {
            installed += t
                .trim_start_matches("Successfully installed")
                .split_whitespace()
                .count();
        } else if t.to_uppercase().starts_with("WARNING") || t.to_uppercase().starts_with("ERROR") {
            warnings.push(line.to_string());
        }
    }

    let mut out: Vec<String> = warnings;
    if installed > 0 {
        out.push(format!("[pip install complete — {} packages]", installed));
    } else {
        let summary: Vec<&str> = output
            .lines()
            .filter(|l| {
                let t = l.trim();
                t.contains("already satisfied")
                    || t.contains("Requirement already")
                    || t.to_uppercase().starts_with("ERROR")
            })
            .take(5)
            .collect();
        if !summary.is_empty() {
            out.extend(summary.iter().map(|l| l.to_string()));
        } else {
            return output.to_string();
        }
    }
    out.join("\n")
}

fn filter_uv_install(output: &str) -> String {
    const UV_SATISFIED_RULES: &[util::MatchOutputRule] = &[util::MatchOutputRule {
        success_pattern: r"(?i)Audited \d+ packages in",
        error_pattern: r"(?i)error|failed",
        ok_message: "ok (up to date)",
    }];
    if let Some(msg) = util::check_match_output(output, UV_SATISFIED_RULES) {
        return msg;
    }

    // uv outputs: "Resolved N packages", "Prepared N packages", "Installed N packages", "Audited N packages"
    let mut warnings: Vec<String> = Vec::new();
    let mut installed = 0usize;
    let mut resolved = 0usize;

    for line in output.lines() {
        let t = line.trim();
        if t.starts_with("Installed ") && t.contains("package") {
            if let Some(n) = t
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse::<usize>().ok())
            {
                installed += n;
            }
        } else if t.starts_with("Resolved ") && t.contains("package") {
            if let Some(n) = t
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse::<usize>().ok())
            {
                resolved += n;
            }
        } else if t.starts_with("error") || t.starts_with("warning") || t.starts_with("  x ") {
            warnings.push(line.to_string());
        }
        // Drop: progress bars, "Downloading", "Building", "Audited"
    }

    let mut out: Vec<String> = warnings;
    if installed > 0 {
        out.push(format!(
            "[uv install complete — {} packages installed, {} resolved]",
            installed, resolved
        ));
    } else if resolved > 0 {
        out.push(format!("[uv: {} packages already satisfied]", resolved));
    } else {
        return output.to_string();
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handler() -> PipHandler {
        PipHandler
    }

    #[test]
    fn pip_already_satisfied_short_circuits() {
        let output =
            "Requirement already satisfied: requests in /usr/lib/python3/dist-packages (2.28.0)";
        let result = handler().filter(
            output,
            &[
                "pip".to_string(),
                "install".to_string(),
                "requests".to_string(),
            ],
        );
        assert_eq!(result, "ok (already satisfied)");
    }

    #[test]
    fn uv_audited_short_circuits() {
        let output = "Resolved 42 packages in 0.05s\nAudited 42 packages in 0.1s";
        let result = handler().filter(output, &["uv".to_string(), "install".to_string()]);
        assert_eq!(result, "ok (up to date)");
    }

    #[test]
    fn pip_actual_install_not_short_circuited() {
        let output = "Collecting requests\n  Downloading requests-2.31.0-py3-none-any.whl (62 kB)\nSuccessfully installed requests-2.31.0";
        let result = handler().filter(
            output,
            &[
                "pip".to_string(),
                "install".to_string(),
                "requests".to_string(),
            ],
        );
        assert_ne!(result, "ok (already satisfied)");
        assert!(result.contains("pip install complete") || result.contains("requests"));
    }

    #[test]
    fn poetry_no_changes_short_circuits() {
        let output = "No dependencies to install or update.";
        let result = handler().filter(output, &["poetry".to_string(), "install".to_string()]);
        assert_eq!(result, "ok (up to date)");
    }

    #[test]
    fn poetry_zero_ops_short_circuits() {
        let output = "Package operations: 0 installs, 0 updates, 0 removals\n";
        let result = handler().filter(output, &["poetry".to_string(), "install".to_string()]);
        assert_eq!(result, "ok (up to date)");
    }

    #[test]
    fn poetry_actual_install_shows_summary() {
        let output = "\
Installing dependencies from lock file

Package operations: 3 installs, 1 update, 0 removals

  - Updating certifi (2023.7.22 -> 2024.2.2)
  - Installing charset-normalizer (3.3.2)
  - Installing idna (3.6)
  - Installing requests (2.31.0)
";
        let result = handler().filter(output, &["poetry".to_string(), "install".to_string()]);
        assert!(result.contains("Package operations: 3 installs"), "should show summary line");
        assert!(!result.contains("Installing charset-normalizer"), "should drop individual install lines");
    }

    #[test]
    fn poetry_error_not_short_circuited() {
        let output = "\
No dependencies to install or update.
Error: Could not find a matching version of package foo
";
        let result = handler().filter(output, &["poetry".to_string(), "install".to_string()]);
        // Error present → short-circuit should NOT fire, raw output preserved
        assert_ne!(result, "ok (up to date)");
    }
}
