use once_cell::sync::OnceCell;
use regex::Regex;

// ── Compiled patterns ─────────────────────────────────────────────────────────

static RE_RUST_PANIC: OnceCell<Regex> = OnceCell::new();
static RE_RUST_BACKTRACE_HEADER: OnceCell<Regex> = OnceCell::new();
static RE_RUST_FRAME: OnceCell<Regex> = OnceCell::new();

static RE_PYTHON_TRACEBACK: OnceCell<Regex> = OnceCell::new();
static RE_PYTHON_FRAME: OnceCell<Regex> = OnceCell::new();
static RE_PYTHON_EXCEPTION: OnceCell<Regex> = OnceCell::new();

static RE_JS_FRAME: OnceCell<Regex> = OnceCell::new();
static RE_JS_ERROR: OnceCell<Regex> = OnceCell::new();

static RE_JAVA_EXCEPTION: OnceCell<Regex> = OnceCell::new();
static RE_JAVA_FRAME: OnceCell<Regex> = OnceCell::new();
static RE_JAVA_CAUSED_BY: OnceCell<Regex> = OnceCell::new();

static RE_GO_GOROUTINE: OnceCell<Regex> = OnceCell::new();
static RE_GO_FRAME: OnceCell<Regex> = OnceCell::new();

fn re_rust_panic() -> &'static Regex {
    RE_RUST_PANIC.get_or_init(|| Regex::new(r"^thread '.+' panicked at").unwrap())
}
fn re_rust_backtrace_header() -> &'static Regex {
    RE_RUST_BACKTRACE_HEADER.get_or_init(|| Regex::new(r"^stack backtrace:").unwrap())
}
fn re_rust_frame() -> &'static Regex {
    RE_RUST_FRAME.get_or_init(|| Regex::new(r"^\s+\d+:\s+\S").unwrap())
}
fn re_python_traceback() -> &'static Regex {
    RE_PYTHON_TRACEBACK.get_or_init(|| Regex::new(r"^Traceback \(most recent call last\):").unwrap())
}
fn re_python_frame() -> &'static Regex {
    RE_PYTHON_FRAME.get_or_init(|| Regex::new(r#"^\s+File ".+", line \d+"#).unwrap())
}
fn re_python_exception() -> &'static Regex {
    RE_PYTHON_EXCEPTION.get_or_init(|| Regex::new(r"^[A-Za-z][\w.]*(?:Error|Exception|Warning):").unwrap())
}
fn re_js_frame() -> &'static Regex {
    RE_JS_FRAME.get_or_init(|| Regex::new(r"^\s+at (?:.+ \(.+:\d+:\d+\)|/\S+:\d+:\d+)").unwrap())
}
fn re_js_error() -> &'static Regex {
    RE_JS_ERROR.get_or_init(|| Regex::new(r"^(?:\w+Error|Error): ").unwrap())
}
fn re_java_exception() -> &'static Regex {
    RE_JAVA_EXCEPTION.get_or_init(|| Regex::new(r"^(?:Exception in thread|[A-Za-z][\w.]+(?:Exception|Error)):").unwrap())
}
fn re_java_frame() -> &'static Regex {
    RE_JAVA_FRAME.get_or_init(|| Regex::new(r"^\s+at [A-Za-z][\w.$]+\(").unwrap())
}
fn re_java_caused_by() -> &'static Regex {
    RE_JAVA_CAUSED_BY.get_or_init(|| Regex::new(r"^Caused by:").unwrap())
}
fn re_go_goroutine() -> &'static Regex {
    RE_GO_GOROUTINE.get_or_init(|| Regex::new(r"^goroutine \d+ \[").unwrap())
}
fn re_go_frame() -> &'static Regex {
    RE_GO_FRAME.get_or_init(|| Regex::new(r"^\t\S+\.go:\d+").unwrap())
}

// ── Stdlib frame detection ────────────────────────────────────────────────────

fn is_stdlib_frame(frame: &str) -> bool {
    let f = frame.trim();
    // JS: node_modules, internal Node APIs
    if f.contains("node_modules/") { return true; }
    if f.starts_with("at internal/") { return true; }
    if f.starts_with("at Object.<anonymous>") { return true; }
    // Java/JVM stdlib packages
    let stripped = if let Some(rest) = f.strip_prefix("at ") { rest } else { f };
    if stripped.starts_with("java.") || stripped.starts_with("javax.") { return true; }
    if stripped.starts_with("sun.") || stripped.starts_with("com.sun.") { return true; }
    if stripped.starts_with("scala.") || stripped.starts_with("kotlin.") { return true; }
    if stripped.starts_with("clojure.") || stripped.starts_with("groovy.") { return true; }
    // Rust stdlib and registry
    if f.contains("/rustc/") { return true; }
    if f.contains("/.cargo/registry/") { return true; }
    if f.contains("rust_begin_unwind") || f.contains("rust_panic") { return true; }
    // Go runtime
    if f.contains("runtime/") && f.contains(".go") { return true; }
    if f.contains("testing.tRunner") { return true; }
    // Python site-packages
    if f.contains("site-packages/") { return true; }
    if f.contains("/lib/python") { return true; }
    false
}

// ── Trace state machine ───────────────────────────────────────────────────────

const MAX_USER_FRAMES: usize = 5;

#[derive(Debug)]
enum TraceKind { Rust, Python, Js, Java, Go }

struct TraceBlock {
    kind: TraceKind,
    headers: Vec<String>,
    user_frames: Vec<String>,
    stdlib_count: usize,
    in_backtrace: bool,
}

impl TraceBlock {
    fn new(kind: TraceKind, first_header: String) -> Self {
        Self {
            kind,
            headers: vec![first_header],
            user_frames: Vec::new(),
            stdlib_count: 0,
            in_backtrace: false,
        }
    }

    fn add_frame(&mut self, frame: &str) {
        if is_stdlib_frame(frame) {
            self.stdlib_count += 1;
        } else if self.user_frames.len() < MAX_USER_FRAMES {
            self.user_frames.push(frame.to_string());
        } else {
            self.stdlib_count += 1;
        }
    }

    fn flush(self, out: &mut Vec<String>) {
        for h in &self.headers {
            out.push(h.clone());
        }
        for f in &self.user_frames {
            out.push(f.clone());
        }
        if self.stdlib_count > 0 {
            out.push(format!("  [... {} internal/stdlib frames omitted ...]", self.stdlib_count));
        }
    }
}

/// Compact stack traces in `input`. Lines outside any recognized trace block pass through unchanged.
pub fn compact(input: &str) -> String {
    let trailing_newline = input.ends_with('\n');
    let lines: Vec<&str> = input.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut trace: Option<TraceBlock> = None;
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];

        // ── Check if we start a new trace block ──────────────────────────────
        if trace.is_none() {
            if re_rust_panic().is_match(line) {
                trace = Some(TraceBlock::new(TraceKind::Rust, line.to_string()));
                i += 1;
                continue;
            }
            if re_python_traceback().is_match(line) {
                trace = Some(TraceBlock::new(TraceKind::Python, line.to_string()));
                i += 1;
                continue;
            }
            if re_js_error().is_match(line) && i + 1 < lines.len() && re_js_frame().is_match(lines[i + 1]) {
                trace = Some(TraceBlock::new(TraceKind::Js, line.to_string()));
                i += 1;
                continue;
            }
            if re_java_exception().is_match(line) {
                trace = Some(TraceBlock::new(TraceKind::Java, line.to_string()));
                i += 1;
                continue;
            }
            if re_go_goroutine().is_match(line) {
                trace = Some(TraceBlock::new(TraceKind::Go, line.to_string()));
                i += 1;
                continue;
            }
            // No trace active: pass through
            out.push(line.to_string());
            i += 1;
            continue;
        }

        // ── We are inside a trace block ──────────────────────────────────────
        let tb = trace.as_mut().unwrap();

        match tb.kind {
            TraceKind::Rust => {
                if re_rust_backtrace_header().is_match(line) {
                    tb.in_backtrace = true;
                    i += 1;
                    continue;
                }
                if tb.in_backtrace && re_rust_frame().is_match(line) {
                    tb.add_frame(line);
                    i += 1;
                    continue;
                }
                if line.trim().starts_with("note:") || line.trim().is_empty() && tb.in_backtrace {
                    i += 1;
                    continue;
                }
                // End of rust trace
                let finished = trace.take().unwrap();
                finished.flush(&mut out);
                // Don't increment i; re-process this line
                continue;
            }
            TraceKind::Python => {
                if re_python_frame().is_match(line) {
                    // Frame line — the next line is the code snippet, skip it
                    tb.add_frame(line);
                    // Skip the code snippet line that follows
                    i += 1;
                    if i < lines.len() && !re_python_frame().is_match(lines[i])
                        && !re_python_exception().is_match(lines[i])
                        && !lines[i].starts_with("Traceback")
                    {
                        // It's the code line; skip
                        i += 1;
                    }
                    continue;
                }
                if re_python_exception().is_match(line) {
                    tb.headers.push(line.to_string());
                    i += 1;
                    // Exception may span multiple lines (chained)
                    while i < lines.len() && !lines[i].is_empty() && !re_python_traceback().is_match(lines[i]) && !re_python_frame().is_match(lines[i]) {
                        tb.headers.push(lines[i].to_string());
                        i += 1;
                    }
                    let finished = trace.take().unwrap();
                    finished.flush(&mut out);
                    continue;
                }
                if line.is_empty() {
                    let finished = trace.take().unwrap();
                    finished.flush(&mut out);
                    out.push(String::new());
                    i += 1;
                    continue;
                }
                // Something unexpected — end trace
                let finished = trace.take().unwrap();
                finished.flush(&mut out);
                continue;
            }
            TraceKind::Js => {
                if re_js_frame().is_match(line) {
                    tb.add_frame(line);
                    i += 1;
                    continue;
                }
                // End of JS trace
                let finished = trace.take().unwrap();
                finished.flush(&mut out);
                continue;
            }
            TraceKind::Java => {
                if re_java_frame().is_match(line) {
                    tb.add_frame(line);
                    i += 1;
                    continue;
                }
                if re_java_caused_by().is_match(line) {
                    tb.headers.push(line.to_string());
                    i += 1;
                    continue;
                }
                if line.trim_start().starts_with("...") {
                    // "... N more" continuation
                    i += 1;
                    continue;
                }
                // End of Java trace
                let finished = trace.take().unwrap();
                finished.flush(&mut out);
                continue;
            }
            TraceKind::Go => {
                if re_go_frame().is_match(line) || line.starts_with('\t') {
                    tb.add_frame(line);
                    i += 1;
                    continue;
                }
                if line.is_empty() {
                    let finished = trace.take().unwrap();
                    finished.flush(&mut out);
                    out.push(String::new());
                    i += 1;
                    continue;
                }
                // End of goroutine
                let finished = trace.take().unwrap();
                finished.flush(&mut out);
                continue;
            }
        }
    }

    // Flush any in-progress trace at end of input
    if let Some(finished) = trace.take() {
        finished.flush(&mut out);
    }

    let mut result = out.join("\n");
    if trailing_newline && !result.is_empty() {
        result.push('\n');
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_panic_compacted() {
        let input = concat!(
            "thread 'main' panicked at 'index out of bounds: the len is 3 but the index is 5', src/main.rs:10:5\n",
            "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n",
            "stack backtrace:\n",
            "   0: rust_begin_unwind\n",
            "   1: core::panicking::panic_fmt\n",
            "   2: myapp::process_data\n",
            "   3: myapp::main\n",
            "   4: std::sys_common::backtrace::__rust_begin_short_backtrace\n",
        );
        let result = compact(input);
        assert!(result.contains("panicked at"), "header lost");
        assert!(result.contains("myapp::process_data") || result.contains("myapp::main"), "user frame lost");
        assert!(result.contains("internal/stdlib frames omitted") || !result.contains("rust_begin_unwind"), "stdlib frame not omitted");
    }

    #[test]
    fn python_traceback_compacted() {
        let input = concat!(
            "Traceback (most recent call last):\n",
            "  File \"/app/main.py\", line 42, in process\n",
            "    result = compute(data)\n",
            "  File \"/usr/lib/python3.10/functools.py\", line 888, in wrapper\n",
            "    return func(*args, **kwargs)\n",
            "  File \"/app/utils.py\", line 15, in compute\n",
            "    return data[\"key\"]\n",
            "KeyError: 'key'\n",
        );
        let result = compact(input);
        assert!(result.contains("Traceback"), "header lost");
        assert!(result.contains("KeyError"), "exception lost");
        assert!(result.contains("/app/"), "user frame lost");
        // Python stdlib frame should be omitted
        assert!(!result.contains("functools.py") || result.contains("omitted"), "stdlib not omitted");
    }

    #[test]
    fn js_node_error_compacted() {
        let input = concat!(
            "TypeError: Cannot read property 'foo' of null\n",
            "    at Object.handler (server.js:42:5)\n",
            "    at /app/node_modules/express/lib/router/route.js:137:13\n",
            "    at Layer.handle [as handle_request] (/app/node_modules/express/lib/router/layer.js:95:5)\n",
        );
        let result = compact(input);
        assert!(result.contains("TypeError"), "error header lost");
        assert!(result.contains("server.js:42"), "user frame lost");
        // node_modules frames should be omitted
        assert!(!result.contains("express/lib") || result.contains("omitted"), "node_modules not omitted");
    }

    #[test]
    fn java_exception_compacted() {
        let input = concat!(
            "Exception in thread \"main\" java.lang.NullPointerException: Cannot invoke method\n",
            "\tat com.myapp.service.UserService.getUser(UserService.java:55)\n",
            "\tat com.myapp.controller.UserController.handle(UserController.java:32)\n",
            "\tat java.base/java.lang.reflect.Method.invoke(Method.java:568)\n",
            "\tat sun.reflect.NativeMethodAccessorImpl.invoke0(Native Method)\n",
        );
        let result = compact(input);
        assert!(result.contains("NullPointerException"), "header lost");
        assert!(result.contains("UserService.java") || result.contains("UserController.java"), "user frame lost");
    }

    #[test]
    fn go_goroutine_compacted() {
        let input = concat!(
            "goroutine 1 [running]:\n",
            "main.processRequest(0xc000012a00, 0x1)\n",
            "\t/home/user/project/main.go:45 +0x1a4\n",
            "net/http.(*ServeMux).ServeHTTP(0x..., 0x...)\n",
            "\t/usr/local/go/src/net/http/server.go:2550 +0x7a\n",
            "\n",
        );
        let result = compact(input);
        assert!(result.contains("goroutine 1"), "header lost");
        assert!(result.contains("main.go:45") || result.contains("processRequest"), "user frame lost");
    }

    #[test]
    fn no_false_positive_on_rust_compiler_error() {
        let input = concat!(
            "error[E0308]: mismatched types\n",
            "  --> src/main.rs:10:5\n",
            "   |\n",
            "10 |     let x: i32 = \"hello\";\n",
            "   |                  ^^^^^^^ expected `i32`, found `&str`\n",
        );
        let result = compact(input);
        assert_eq!(result, input, "rust compiler error should be unchanged");
    }

    #[test]
    fn passthrough_non_trace_output() {
        let input = "Building project...\nCompilation successful\nTests passed: 42\n";
        assert_eq!(compact(input), input);
    }

    #[test]
    fn empty_input_unchanged() {
        assert_eq!(compact(""), "");
    }

    #[test]
    fn trace_at_end_of_file_flushed() {
        let input = concat!(
            "goroutine 1 [running]:\n",
            "main.main()\n",
            "\t/home/user/main.go:10 +0x25",
        );
        let result = compact(input);
        assert!(result.contains("goroutine 1"));
        assert!(result.contains("main.go:10") || result.contains("main.main"));
    }
}
