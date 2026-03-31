use once_cell::sync::OnceCell;
use regex::Regex;
use crate::handlers::Handler;

static RE_TASK_HEADER: OnceCell<Regex> = OnceCell::new();
static RE_NX_CLOUD: OnceCell<Regex> = OnceCell::new();
static RE_CACHE_HIT: OnceCell<Regex> = OnceCell::new();
static RE_SEPARATOR: OnceCell<Regex> = OnceCell::new();
static RE_NX_SUMMARY: OnceCell<Regex> = OnceCell::new();
static RE_ERROR_LINE: OnceCell<Regex> = OnceCell::new();

fn re_task_header() -> &'static Regex {
    RE_TASK_HEADER.get_or_init(|| Regex::new(r"^> nx run ([\w/@-]+):([\w-]+)").unwrap())
}
fn re_nx_cloud() -> &'static Regex {
    RE_NX_CLOUD.get_or_init(|| Regex::new(r"(?i)NX\s+Cloud|NX\s+Nx Cloud").unwrap())
}
fn re_cache_hit() -> &'static Regex {
    RE_CACHE_HIT.get_or_init(|| Regex::new(r"\[(local cache|remote cache)\]").unwrap())
}
fn re_separator() -> &'static Regex {
    RE_SEPARATOR.get_or_init(|| Regex::new(r"^[-─]{4,}$").unwrap())
}
fn re_nx_summary() -> &'static Regex {
    RE_NX_SUMMARY.get_or_init(|| {
        Regex::new(r"^(NX\s+(Successfully ran|Failed to run)|\s*Ran \d+ tasks?|NX\s+\d+ task)").unwrap()
    })
}
fn re_error_line() -> &'static Regex {
    RE_ERROR_LINE.get_or_init(|| Regex::new(r"(?i)(error|failed|fail:|FAILED|ERROR)").unwrap())
}

pub struct NxHandler;

impl Handler for NxHandler {
    fn rewrite_args(&self, args: &[String]) -> Vec<String> {
        if !args.iter().any(|a| a.contains("--output-style")) {
            let mut out = args.to_vec();
            out.push("--output-style=stream".to_string());
            out
        } else {
            args.to_vec()
        }
    }

    fn filter(&self, output: &str, _args: &[String]) -> String {
        filter_nx(output)
    }
}

struct PassingGroup {
    count: usize,
    cached: usize,
}

impl PassingGroup {
    fn flush(&mut self, out: &mut Vec<String>) {
        if self.count == 0 { return; }
        if self.cached == self.count {
            out.push(format!("[{} task{} passed (all cached)]",
                self.count, if self.count == 1 { "" } else { "s" }));
        } else {
            out.push(format!("[{} task{} passed ({} cached)]",
                self.count, if self.count == 1 { "" } else { "s" }, self.cached));
        }
        self.count = 0;
        self.cached = 0;
    }
}

fn filter_nx(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();

    // If there are no nx task headers, return as-is (not nx structured output)
    if !lines.iter().any(|l| re_task_header().is_match(l)) {
        return output.to_string();
    }

    let mut out: Vec<String> = Vec::new();
    let mut passing = PassingGroup { count: 0, cached: 0 };
    let mut in_task = false;
    let mut task_is_failing = false;
    let mut task_lines: Vec<String> = Vec::new();
    let mut current_task_header = String::new();

    for line in &lines {
        // Always strip Nx Cloud and separator lines
        if re_nx_cloud().is_match(line) { continue; }
        if re_separator().is_match(line) { continue; }

        // Summary lines: flush everything and emit verbatim
        if re_nx_summary().is_match(line) {
            if in_task {
                if task_is_failing {
                    passing.flush(&mut out);
                    out.push(current_task_header.clone());
                    out.extend(task_lines.drain(..));
                } else {
                    passing.count += 1;
                    if re_cache_hit().is_match(&current_task_header) || task_lines.iter().any(|l| re_cache_hit().is_match(l)) {
                        passing.cached += 1;
                    }
                    task_lines.clear();
                }
                in_task = false;
            }
            passing.flush(&mut out);
            out.push(line.to_string());
            continue;
        }

        // New task header
        if re_task_header().is_match(line) {
            // Close previous task
            if in_task {
                if task_is_failing {
                    passing.flush(&mut out);
                    out.push(current_task_header.clone());
                    out.extend(task_lines.drain(..));
                } else {
                    passing.count += 1;
                    if re_cache_hit().is_match(&current_task_header) || task_lines.iter().any(|l| re_cache_hit().is_match(l)) {
                        passing.cached += 1;
                    }
                    task_lines.clear();
                }
            }
            in_task = true;
            task_is_failing = false;
            current_task_header = line.to_string();
            task_lines.clear();
            continue;
        }

        if in_task {
            if re_cache_hit().is_match(line) {
                passing.cached += 1; // will be incremented when task closes
                // Don't count it twice — we'll check task_lines at close
                task_lines.push(line.to_string());
                continue;
            }
            if re_error_line().is_match(line) {
                task_is_failing = true;
            }
            task_lines.push(line.to_string());
        } else {
            out.push(line.to_string());
        }
    }

    // Flush final task
    if in_task {
        if task_is_failing {
            passing.flush(&mut out);
            out.push(current_task_header);
            out.extend(task_lines);
        } else {
            passing.count += 1;
            task_lines.clear();
        }
    }
    passing.flush(&mut out);

    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_task(project: &str, target: &str, lines: &[&str], cached: bool) -> String {
        let mut v = vec![format!("> nx run {}:{}", project, target)];
        if cached { v.push("[local cache]".to_string()); }
        v.extend(lines.iter().map(|l| l.to_string()));
        v.join("\n")
    }

    #[test]
    fn passing_tasks_collapsed() {
        let input = format!("{}\n{}\nNX  Successfully ran 2 tasks\n",
            make_task("app", "build", &["Compiled successfully"], false),
            make_task("lib", "test", &["All tests passed"], false),
        );
        let handler = NxHandler;
        let result = handler.filter(&input, &[]);
        assert!(result.contains("2 tasks passed"), "tasks not collapsed: {}", result);
        assert!(result.contains("Successfully ran"), "summary missing");
        assert!(!result.contains("> nx run"), "task header not collapsed");
    }

    #[test]
    fn failing_task_output_preserved() {
        let input = format!("{}\n{}\nNX  Failed to run 1 task\n",
            make_task("app", "build", &["error: build failed"], false),
            make_task("lib", "test", &["All tests passed"], false),
        );
        let handler = NxHandler;
        let result = handler.filter(&input, &[]);
        assert!(result.contains("> nx run app:build"), "failing task header missing");
        assert!(result.contains("build failed"), "failing task error missing");
        assert!(result.contains("1 task passed"), "passing task not collapsed: {}", result);
    }

    #[test]
    fn cached_tasks_counted() {
        let input = format!("{}\n{}\nNX  Successfully ran 2 tasks\n",
            make_task("app", "build", &[], true),
            make_task("lib", "test", &["All tests passed"], false),
        );
        let handler = NxHandler;
        let result = handler.filter(&input, &[]);
        assert!(result.contains("cached"), "cached count missing: {}", result);
    }

    #[test]
    fn nx_cloud_lines_stripped() {
        let input = "NX   Nx Cloud enabled\n> nx run app:build\nBuilt ok\nNX  Successfully ran 1 task\n";
        let handler = NxHandler;
        let result = handler.filter(input, &[]);
        assert!(!result.contains("Nx Cloud"), "nx cloud not stripped");
    }

    #[test]
    fn separator_lines_stripped() {
        let input = "-------------------------------------------\n> nx run app:test\nAll tests pass\nNX  Successfully ran 1 task\n";
        let handler = NxHandler;
        let result = handler.filter(input, &[]);
        assert!(!result.contains("---"), "separator not stripped");
    }

    #[test]
    fn rewrite_args_injects_output_style() {
        let handler = NxHandler;
        let args = vec!["nx".to_string(), "run".to_string(), "app:build".to_string()];
        let result = handler.rewrite_args(&args);
        assert!(result.iter().any(|a| a.contains("output-style")), "output-style not injected");
    }

    #[test]
    fn rewrite_args_no_duplicate() {
        let handler = NxHandler;
        let args = vec!["nx".to_string(), "run".to_string(), "--output-style=static".to_string()];
        let result = handler.rewrite_args(&args);
        let count = result.iter().filter(|a| a.contains("output-style")).count();
        assert_eq!(count, 1, "duplicate output-style");
    }

    #[test]
    fn passthrough_when_no_nx_structure() {
        let input = "some random output\nwith no nx task headers\n";
        let handler = NxHandler;
        let result = handler.filter(input, &[]);
        assert_eq!(result, input, "should pass through non-nx output");
    }
}
