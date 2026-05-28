use super::Handler;

pub struct RsyncHandler;

impl Handler for RsyncHandler {
    fn filter(&self, output: &str, _args: &[String]) -> String {
        let lines: Vec<&str> = output.lines().collect();
        if lines.len() < 5 {
            return output.to_string();
        }

        let mut result: Vec<String> = Vec::new();
        let mut max_xfr: usize = 0;

        for line in &lines {
            // Drop carriage-return progress lines (overwritten in terminal)
            if line.contains('\r') {
                continue;
            }
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            // Extract max xfr# value before dropping progress lines.
            // Format: "(xfr#N, to-chk=M/T)" or "xfr#N, to-chk=..."
            if t.contains("xfr#") {
                if let Some(n) = extract_xfr_num(t) {
                    if n > max_xfr {
                        max_xfr = n;
                    }
                }
            }
            // Drop transfer speed/progress lines: "   1,048,576  10%    1.00MB/s    0:00:09"
            if is_progress_line(t) {
                continue;
            }
            result.push(line.to_string());
        }

        // Append file count summary if we saw any xfr# progress lines
        if max_xfr > 0 {
            result.push(format!("[{} files transferred]", max_xfr));
        }

        if result.is_empty() {
            output.to_string()
        } else {
            result.join("\n")
        }
    }
}

/// Extract the transfer count N from a line containing "xfr#N".
fn extract_xfr_num(line: &str) -> Option<usize> {
    let start = line.find("xfr#")? + 4;
    let rest = &line[start..];
    let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn is_progress_line(line: &str) -> bool {
    // Intermediate progress: "   1,048,576  10%    1.00MB/s    0:00:09"
    (line.contains('%') && (line.contains("MB/s") || line.contains("kB/s") || line.contains("GB/s")))
        // Incremental info lines: "to-chk=45/100"
        || line.contains("to-chk=")
        // Transfer count lines: "xfr#3, to-chk=45/100"
        || line.contains("xfr#")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::Handler;

    fn handler() -> RsyncHandler {
        RsyncHandler
    }

    #[test]
    fn progress_lines_dropped() {
        let output = "\
sending incremental file list
./
file1.txt
      1,048,576  50%    1.00MB/s    0:00:01  xfr#1, to-chk=2/4
      2,097,152 100%    2.00MB/s    0:00:00 (xfr#2, to-chk=0/4)
sent 2,097,400 bytes  received 35 bytes  1,398,290.00 bytes/sec
total size is 2,097,152  speedup is 1.00
";
        let result = handler().filter(output, &[]);
        assert!(!result.contains("MB/s"), "should drop progress lines");
        assert!(!result.contains("to-chk="), "should drop to-chk lines");
        assert!(result.contains("sent 2,097,400 bytes"), "should keep summary line");
        assert!(result.contains("file1.txt"), "should keep file list");
        assert!(result.contains("[2 files transferred]"), "should add xfr count: got {:?}", result);
    }

    #[test]
    fn short_output_passes_through() {
        let output = "sent 100 bytes  received 12 bytes\n";
        let result = handler().filter(output, &[]);
        assert_eq!(result, output);
    }

    #[test]
    fn summary_line_kept() {
        let output = "\
sending incremental file list
a.txt
b.txt
c.txt
d.txt
e.txt
      512  100%    0.00kB/s    0:00:00
sent 1,234 bytes  received 56 bytes  2,580.00 bytes/sec
total size is 1,024  speedup is 0.83
";
        let result = handler().filter(output, &[]);
        assert!(result.contains("sent 1,234 bytes"), "summary should be present");
        assert!(result.contains("total size"), "total line should be present");
    }
}
