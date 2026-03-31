/// Returns true if >50% of non-empty lines are valid JSON objects.
pub fn is_json_log(input: &str) -> bool {
    let lines: Vec<&str> = input.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() < 3 {
        return false;
    }
    let json_count = lines
        .iter()
        .filter(|l| {
            let t = l.trim();
            t.starts_with('{') && serde_json::from_str::<serde_json::Value>(t).is_ok()
        })
        .count();
    (json_count as f64 / lines.len() as f64) > 0.50
}

const LEVEL_KEYS: &[&str] = &["level", "severity", "lvl", "log_level", "loglevel"];
const MESSAGE_KEYS: &[&str] = &["message", "msg", "text", "body", "log"];
const ERROR_KEYS: &[&str] = &["error", "err", "exception", "cause"];

fn normalize_level(raw: &str) -> &'static str {
    match raw.to_uppercase().as_str() {
        "ERROR" | "ERR" | "FATAL" | "CRITICAL" | "CRIT" | "PANIC" => "ERROR",
        "WARN" | "WARNING" | "WRN" => "WARN",
        "INFO" | "INF" | "INFORMATION" => "INFO",
        "DEBUG" | "DBG" | "DEBU" => "DEBUG",
        "TRACE" | "TRC" | "VERBOSE" => "TRACE",
        _ => "INFO",
    }
}

fn extract_string_field(obj: &serde_json::Map<String, serde_json::Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(v) = obj.get(*key) {
            match v {
                serde_json::Value::String(s) => return Some(s.clone()),
                serde_json::Value::Number(n) => return Some(n.to_string()),
                _ => {}
            }
        }
    }
    None
}

struct InfoCollapser {
    last_prefix: String,
    run_count: usize,
}

impl InfoCollapser {
    fn new() -> Self {
        Self { last_prefix: String::new(), run_count: 0 }
    }

    fn feed(&mut self, msg: &str, out: &mut Vec<String>) {
        let prefix: String = msg.chars().take(20).collect();
        if prefix == self.last_prefix {
            self.run_count += 1;
            if self.run_count <= 3 {
                out.push(format!("[INFO] {}", msg));
            }
            // beyond 3, suppress until prefix changes
        } else {
            self.flush(out);
            self.last_prefix = prefix;
            self.run_count = 1;
            out.push(format!("[INFO] {}", msg));
        }
    }

    fn flush(&mut self, out: &mut Vec<String>) {
        if self.run_count > 3 {
            out.push(format!("[{} similar INFO lines omitted]", self.run_count - 3));
        }
        self.run_count = 0;
        self.last_prefix.clear();
    }
}

/// Compact JSON-per-line log output into human-readable form.
/// If input is not a JSON log, returns it unchanged.
pub fn compact(input: &str) -> String {
    if !is_json_log(input) {
        return input.to_string();
    }

    let mut out: Vec<String> = Vec::new();
    let mut info_collapser = InfoCollapser::new();

    for line in input.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            info_collapser.flush(&mut out);
            continue;
        }

        // Try to parse as JSON object
        if trimmed.starts_with('{') {
            if let Ok(serde_json::Value::Object(obj)) = serde_json::from_str(trimmed) {
                let level_raw = extract_string_field(&obj, LEVEL_KEYS)
                    .unwrap_or_else(|| "INFO".to_string());
                let level = normalize_level(&level_raw);
                let message = extract_string_field(&obj, MESSAGE_KEYS)
                    .unwrap_or_else(|| trimmed.chars().take(80).collect());
                let error = extract_string_field(&obj, ERROR_KEYS);

                match level {
                    "DEBUG" | "TRACE" => {
                        // drop
                    }
                    "INFO" => {
                        info_collapser.feed(&message, &mut out);
                    }
                    "WARN" => {
                        info_collapser.flush(&mut out);
                        out.push(format!("[WARN] {}", message));
                    }
                    "ERROR" => {
                        info_collapser.flush(&mut out);
                        out.push(format!("[ERROR] {}", message));
                        if let Some(err) = error {
                            out.push(format!("  | error: {}", err));
                        }
                    }
                    _ => {
                        info_collapser.flush(&mut out);
                        out.push(format!("[{}] {}", level, message));
                    }
                }
                continue;
            }
        }

        // Non-JSON line: pass through verbatim
        info_collapser.flush(&mut out);
        out.push(line.to_string());
    }

    info_collapser.flush(&mut out);
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json_line(level: &str, msg: &str) -> String {
        format!(r#"{{"level":"{}","msg":"{}","ts":1234567890}}"#, level, msg)
    }

    fn json_line_with_error(level: &str, msg: &str, err: &str) -> String {
        format!(r#"{{"level":"{}","msg":"{}","error":"{}"}}"#, level, msg, err)
    }

    #[test]
    fn detects_json_log_true() {
        let input: Vec<String> = (0..10).map(|i| json_line("info", &format!("request {}", i))).collect();
        assert!(is_json_log(&input.join("\n")));
    }

    #[test]
    fn detects_json_log_false_below_50pct() {
        let mut lines = vec![
            json_line("info", "msg1"),
            json_line("info", "msg2"),
        ];
        lines.extend((0..5).map(|i| format!("plain text line {}", i)));
        assert!(!is_json_log(&lines.join("\n")));
    }

    #[test]
    fn detects_json_log_false_too_short() {
        let lines = vec![json_line("info", "msg1"), json_line("info", "msg2")];
        assert!(!is_json_log(&lines.join("\n")));
    }

    #[test]
    fn drops_debug_and_trace_lines() {
        let input = vec![
            json_line("debug", "verbose debug info"),
            json_line("trace", "very verbose"),
            json_line("error", "something failed"),
        ];
        let result = compact(&input.join("\n"));
        assert!(!result.contains("verbose debug"));
        assert!(!result.contains("very verbose"));
        assert!(result.contains("[ERROR] something failed"));
    }

    #[test]
    fn collapses_consecutive_info_lines() {
        let input: Vec<String> = (0..8)
            .map(|i| format!(r#"{{"level":"info","msg":"Processing request id={}", "ts":0}}"#, i))
            .collect();
        let result = compact(&input.join("\n"));
        assert!(result.contains("similar INFO lines omitted") || result.lines().count() < 8);
    }

    #[test]
    fn keeps_all_error_lines() {
        let input: Vec<String> = (0..5)
            .map(|i| json_line("error", &format!("error number {}", i)))
            .collect();
        let result = compact(&input.join("\n"));
        for i in 0..5 {
            assert!(result.contains(&format!("error number {}", i)), "missing error {}", i);
        }
    }

    #[test]
    fn error_field_appended() {
        let input = json_line_with_error("error", "DB query failed", "connection timeout");
        let lines: Vec<String> = std::iter::repeat(json_line("info", "ok"))
            .take(5)
            .chain(std::iter::once(input))
            .collect();
        let result = compact(&lines.join("\n"));
        assert!(result.contains("DB query failed"));
        assert!(result.contains("connection timeout"));
    }

    #[test]
    fn warn_lines_preserved() {
        let lines: Vec<String> = (0..5)
            .map(|i| if i == 2 { json_line("warn", "Retrying connection") } else { json_line("info", "ok") })
            .collect();
        let result = compact(&lines.join("\n"));
        assert!(result.contains("[WARN] Retrying connection"));
    }

    #[test]
    fn non_json_lines_pass_through() {
        let mut lines: Vec<String> = (0..8).map(|i| json_line("info", &format!("msg {}", i))).collect();
        lines.insert(3, "plain text line here".to_string());
        let result = compact(&lines.join("\n"));
        assert!(result.contains("plain text line here"));
    }

    #[test]
    fn compact_returns_unchanged_when_not_json_log() {
        let input = "error: mismatched types\n  --> src/main.rs:10:5\n  |\n10 | let x = 1;";
        assert_eq!(compact(input), input);
    }

    #[test]
    fn empty_input_unchanged() {
        assert_eq!(compact(""), "");
    }

    #[test]
    fn is_json_log_false_for_cargo_output() {
        let input = "   Compiling foo v0.1.0\nerror[E0308]: mismatched types\n  --> src/main.rs:5:10\n   |\n5  |     let x: i32 = \"hello\";\n   |                  ^^^^^^^";
        assert!(!is_json_log(input));
    }

    #[test]
    fn level_normalization() {
        assert_eq!(normalize_level("CRITICAL"), "ERROR");
        assert_eq!(normalize_level("WARNING"), "WARN");
        assert_eq!(normalize_level("DBG"), "DEBUG");
        assert_eq!(normalize_level("INF"), "INFO");
    }
}
