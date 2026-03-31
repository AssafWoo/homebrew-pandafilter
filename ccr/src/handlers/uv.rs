use once_cell::sync::OnceCell;
use regex::Regex;
use crate::handlers::Handler;

static RE_UV_DOWNLOAD: OnceCell<Regex> = OnceCell::new();
static RE_UV_PYTHON_VER: OnceCell<Regex> = OnceCell::new();
static RE_UV_VENV: OnceCell<Regex> = OnceCell::new();
static RE_UV_AUDITED: OnceCell<Regex> = OnceCell::new();
static RE_UV_RESOLVED: OnceCell<Regex> = OnceCell::new();
static RE_UV_ERROR: OnceCell<Regex> = OnceCell::new();

fn re_uv_download() -> &'static Regex {
    RE_UV_DOWNLOAD.get_or_init(|| Regex::new(r"(?i)^\s*(Downloading|Fetching|Preparing)\s+\S+").unwrap())
}
fn re_uv_python_ver() -> &'static Regex {
    RE_UV_PYTHON_VER.get_or_init(|| Regex::new(r"^Using (CPython|PyPy|Python) \d+\.\d+").unwrap())
}
fn re_uv_venv() -> &'static Regex {
    RE_UV_VENV.get_or_init(|| Regex::new(r"^Creating virtual environment").unwrap())
}
fn re_uv_audited() -> &'static Regex {
    RE_UV_AUDITED.get_or_init(|| Regex::new(r"^\s*Audited \d+").unwrap())
}
fn re_uv_resolved() -> &'static Regex {
    RE_UV_RESOLVED.get_or_init(|| Regex::new(r"^\s*Resolved \d+").unwrap())
}
fn re_uv_error() -> &'static Regex {
    RE_UV_ERROR.get_or_init(|| Regex::new(r"(?i)^\s*(error|warning|  x |\berror\b)").unwrap())
}

pub struct UvHandler;

impl Handler for UvHandler {
    fn rewrite_args(&self, args: &[String]) -> Vec<String> {
        args.to_vec()
    }

    fn filter(&self, output: &str, args: &[String]) -> String {
        let subcmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
        match subcmd {
            "install" | "add" | "sync" | "remove" | "lock" => filter_uv_install(output),
            _ => output.to_string(),
        }
    }
}

fn filter_uv_install(output: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut download_count = 0usize;

    for line in output.lines() {
        // Always keep error/warning lines
        if re_uv_error().is_match(line) {
            if download_count > 0 {
                out.push("  [downloads omitted]");
                download_count = 0;
            }
            out.push(line);
            continue;
        }
        // Skip Python version / venv creation / resolved / audited lines
        if re_uv_python_ver().is_match(line)
            || re_uv_venv().is_match(line)
            || re_uv_resolved().is_match(line)
            || re_uv_audited().is_match(line)
        {
            continue;
        }
        // Collapse download lines
        if re_uv_download().is_match(line) {
            download_count += 1;
            continue;
        }
        // Flush any pending download count
        if download_count > 0 {
            out.push("  [downloads omitted]");
            download_count = 0;
        }
        out.push(line);
    }
    if download_count > 0 {
        out.push("  [downloads omitted]");
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uv_strips_download_progress() {
        let input = "Downloading requests-2.31.0 (62 kB)\nDownloading flask-3.0.0 (180 kB)\nInstalled 2 packages in 0.8s";
        let handler = UvHandler;
        let result = handler.filter(input, &["uv".to_string(), "install".to_string()]);
        assert!(!result.contains("Downloading requests"), "download line not stripped");
        assert!(result.contains("Installed"), "install summary removed");
    }

    #[test]
    fn uv_keeps_error_lines() {
        let input = "error: Resolution failed\n  x No version found for package==99.0\nInstalled 0 packages";
        let handler = UvHandler;
        let result = handler.filter(input, &["uv".to_string(), "install".to_string()]);
        assert!(result.contains("error: Resolution failed"));
        assert!(result.contains("No version found"));
    }

    #[test]
    fn uv_strips_python_version_lines() {
        let input = "Using CPython 3.12.0 interpreter at /usr/bin/python3\nResolved 5 packages in 0.1s\nInstalled 5 packages in 0.3s";
        let handler = UvHandler;
        let result = handler.filter(input, &["uv".to_string(), "install".to_string()]);
        assert!(!result.contains("CPython"), "python version line not stripped");
        assert!(result.contains("Installed"), "install summary removed");
    }

    #[test]
    fn uv_strips_resolved_and_audited() {
        let input = "Resolved 10 packages in 0.05s\nAudited 10 packages in 0.01s\nInstalled 10 packages in 0.5s";
        let handler = UvHandler;
        let result = handler.filter(input, &["uv".to_string(), "install".to_string()]);
        assert!(!result.contains("Resolved"), "resolved line not stripped");
        assert!(!result.contains("Audited"), "audited line not stripped");
        assert!(result.contains("Installed"));
    }

    #[test]
    fn uv_sync_treated_same_as_install() {
        let input = "Downloading package-1.0 (5 kB)\nInstalled 1 package in 0.2s";
        let handler = UvHandler;
        let result = handler.filter(input, &["uv".to_string(), "sync".to_string()]);
        assert!(!result.contains("Downloading"), "sync should filter like install");
    }

    #[test]
    fn uv_run_passthrough() {
        let input = "some random output from a script";
        let handler = UvHandler;
        let result = handler.filter(input, &["uv".to_string(), "run".to_string()]);
        assert_eq!(result, input, "uv run should pass through");
    }
}
