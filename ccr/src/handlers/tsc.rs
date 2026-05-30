use std::sync::OnceLock;

use super::Handler;

pub struct TscHandler;

fn re_ts_error() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"^(.+\.tsx?)\((\d+),\d+\):\s+(error|warning)\s+(TS\d+:.+)$")
            .expect("tsc error regex")
    })
}

/// Returns true if the TS error code in the message is in the 5xxx range (config errors).
fn is_ts5xxx(msg: &str) -> bool {
    // msg looks like "TS5055: Cannot write file ..."
    let code_str = msg.split(':').next().unwrap_or("").trim();
    if let Some(digits) = code_str.strip_prefix("TS") {
        if let Ok(n) = digits.parse::<u32>() {
            return n >= 5000 && n < 6000;
        }
    }
    false
}

/// Maximum length for a TypeScript error message before truncation.
/// TypeScript emits very verbose type mismatch descriptions; trim them to keep context.
const MAX_MSG_LEN: usize = 80;

impl Handler for TscHandler {
    fn rewrite_args(&self, args: &[String]) -> Vec<String> {
        let mut out = args.to_vec();
        // Add --noEmit so TypeScript only type-checks without writing .js files
        if !out.iter().any(|a| a == "--noEmit" || a == "--noemit") {
            out.push("--noEmit".to_string());
        }
        out
    }

    fn filter(&self, output: &str, _args: &[String]) -> String {
        // Clean build
        if output.contains("Found 0 errors") {
            return "Build OK".to_string();
        }

        let lines: Vec<&str> = output.lines().collect();
        let mut error_count = 0usize;
        let mut warning_count = 0usize;

        // Group errors/warnings by file
        // Lines like: src/foo.ts(42,5): error TS2345: ...
        let mut grouped: Vec<(String, Vec<(String, String, String)>)> = Vec::new(); // (file, [(lineno, kind, msg)])
        for line in &lines {
            if let Some(caps) = re_ts_error().captures(line) {
                let file = caps[1].to_string();
                let lineno = caps[2].to_string();
                let kind = caps[3].to_string();
                let raw_msg = caps[4].to_string();

                // Truncate verbose type error messages
                let msg = if raw_msg.len() > MAX_MSG_LEN {
                    format!("{}…", &raw_msg[..MAX_MSG_LEN])
                } else {
                    raw_msg
                };

                if kind == "error" {
                    error_count += 1;
                } else {
                    warning_count += 1;
                }

                if let Some(last) = grouped.last_mut() {
                    if last.0 == file {
                        last.1.push((lineno, kind, msg));
                        continue;
                    }
                }
                grouped.push((file, vec![(lineno, kind, msg)]));
            }
        }

        if grouped.is_empty() {
            return output.to_string();
        }

        let mut out: Vec<String> = Vec::new();

        // Collect all TS5xxx messages across all files for a single grouped summary.
        let mut ts5_count = 0usize;

        for (file, messages) in &grouped {
            // Separate TS5xxx from non-TS5xxx within this file's messages.
            let non5: Vec<&(String, String, String)> =
                messages.iter().filter(|(_, _, m)| !is_ts5xxx(m)).collect();
            let five_count = messages.iter().filter(|(_, _, m)| is_ts5xxx(m)).count();
            ts5_count += five_count;

            // Only emit the file header if there are non-TS5xxx diagnostics.
            if !non5.is_empty() {
                out.push(file.clone());

                // Within each file, collapse runs of the same TS error code.
                // e.g. TS2339 appearing 4 times → "  TS2339 (×4): L12, L45, L78, L92 — msg"
                let mut i = 0;
                while i < non5.len() {
                    let (lineno, kind, msg) = non5[i];
                    // Extract the TS code prefix (e.g. "TS2339")
                    let ts_code = msg.split(':').next().unwrap_or("").trim();
                    // Collect consecutive entries with the same code
                    let mut j = i + 1;
                    while j < non5.len() {
                        let (_, k2, m2) = non5[j];
                        let code2 = m2.split(':').next().unwrap_or("").trim();
                        if code2 == ts_code && k2 == kind {
                            j += 1;
                        } else {
                            break;
                        }
                    }
                    let count = j - i;
                    if count == 1 {
                        out.push(format!("  L{}: {} {}", lineno, kind, msg));
                    } else {
                        let line_nums: Vec<String> = non5[i..j]
                            .iter()
                            .map(|(ln, _, _)| format!("L{}", ln))
                            .collect();
                        // Keep the message from the first occurrence (already truncated)
                        let msg_after_code = msg.splitn(2, ':').nth(1).unwrap_or(msg).trim();
                        let msg_preview = if msg_after_code.len() > 60 {
                            format!("{}…", &msg_after_code[..60])
                        } else {
                            msg_after_code.to_string()
                        };
                        out.push(format!(
                            "  {} (×{}): {} — {}",
                            ts_code, count,
                            line_nums.join(", "),
                            msg_preview
                        ));
                    }
                    i = j;
                }
            }
        }

        // Emit a single summary line for all TS5xxx config errors.
        if ts5_count > 0 {
            out.push(format!(
                "[TS5xxx: {} config error{} — run tsc --noEmit to see details]",
                ts5_count,
                if ts5_count == 1 { "" } else { "s" }
            ));
        }

        out.push(format!("[{} errors, {} warnings]", error_count, warning_count));
        out.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::Handler;

    #[test]
    fn ts5xxx_grouped_into_single_summary() {
        // 5 identical TS5055 lines across the same file should collapse to exactly
        // one TS5xxx summary line, not five individual lines.
        let input: String = (1..=5)
            .map(|i| {
                format!(
                    "src/tsconfig.ts({},1): error TS5055: Cannot write file 'dist/foo.js'.\n",
                    i
                )
            })
            .collect();
        let handler = TscHandler;
        let result = handler.filter(&input, &[]);
        let ts5_lines: Vec<&str> = result.lines().filter(|l| l.contains("TS5xxx")).collect();
        assert_eq!(ts5_lines.len(), 1, "expected exactly 1 TS5xxx summary line, got:\n{}", result);
        assert!(ts5_lines[0].contains('5'), "summary should mention the count 5:\n{}", result);
        // No individual TS5055 lines should appear
        assert!(
            !result.contains("TS5055"),
            "individual TS5055 lines should not appear:\n{}",
            result
        );
    }

    #[test]
    fn non_ts5xxx_errors_still_shown_per_file() {
        let input = "src/app.ts(10,3): error TS2345: Argument of type 'string' is not assignable.\n";
        let handler = TscHandler;
        let result = handler.filter(input, &[]);
        assert!(result.contains("src/app.ts"), "file name should appear");
        assert!(result.contains("TS2345"), "non-5xxx error should be shown individually");
        assert!(!result.contains("TS5xxx"), "no TS5xxx summary expected");
    }
}
