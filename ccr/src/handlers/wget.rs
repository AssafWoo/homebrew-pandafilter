use super::Handler;

pub struct WgetHandler;

impl Handler for WgetHandler {
    fn rewrite_args(&self, args: &[String]) -> Vec<String> {
        let mut out = args.to_vec();
        // Add --quiet (suppresses progress/spinner) unless verbose mode is explicitly set
        if !out.iter().any(|a| a == "--quiet" || a == "-q" || a == "--verbose" || a == "-v" || a == "--debug") {
            out.push("--quiet".to_string());
        }
        out
    }

    fn filter(&self, output: &str, _args: &[String]) -> String {
        let important: Vec<&str> = output
            .lines()
            .filter(|l| !is_wget_noise(l))
            .collect();

        if important.is_empty() || important.len() == output.lines().count() {
            return output.to_string();
        }
        important.join("\n")
    }
}

/// Returns true for wget progress / spinner lines that carry no useful information.
fn is_wget_noise(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false; // keep blank lines (structural)
    }

    // wget progress bar pattern: "<filename>   N%[=====...>]  <size>  <speed>  in <time>"
    // Detect by looking for the `%[` sequence (percentage immediately before bracket).
    if t.contains("%[") && t.contains(']') {
        return true;
    }

    // Transfer speed summary line at the end: "N (N MB/s) - '...' saved [N/N]"
    // These are useful but very long; keep them (return false)

    // Lines that are only dots / progress characters (some wget versions use dots)
    if t.len() > 10
        && t.chars().all(|c| matches!(c, '.' | ' ' | 'K' | 'M' | 'G'))
    {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handler() -> WgetHandler {
        WgetHandler
    }

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn filter_strips_progress_lines() {
        let output = "\
--2024-01-01 12:00:00--  https://example.com/file.zip
Resolving example.com... 93.184.216.34
Connecting to example.com|93.184.216.34|:443... connected.
HTTP request sent, awaiting response... 200 OK
Length: 1024000 (1000K) [application/zip]
Saving to: 'file.zip'

file.zip            100%[===================>]   1.00M  5.12MB/s    in 0.2s

2024-01-01 12:00:01 (5.12 MB/s) - 'file.zip' saved [1024000/1024000]
";
        let result = handler().filter(output, &args(&["wget", "https://example.com/file.zip"]));
        assert!(!result.contains("100%[====="), "should strip progress bar");
        assert!(result.contains("200 OK") || result.contains("saved"), "should keep status line");
    }

    #[test]
    fn filter_passthrough_for_short_output() {
        let output = "Error: connection refused\n";
        let result = handler().filter(output, &args(&["wget", "http://localhost"]));
        assert_eq!(result, output);
    }
}
