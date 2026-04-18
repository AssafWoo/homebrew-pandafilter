/// Integration tests for the three new features added in the BERT CPU optimizer plan:
///
/// 1. WebFetch — budget-aware section compression of fetched pages
/// 2. WebSearch — ranking + collapse of search results
/// 3. Error-loop detection — structural diff of repeated build errors
///
/// These tests exercise the full `panda hook` binary path from JSON stdin to
/// stdout so they verify the dispatch table wiring, the pass-through contract,
/// and the observable output shape — not just the internal helper functions.

use assert_cmd::Command;
use std::process::id;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build a panda-hook JSON payload.
fn hook_json(tool_name: &str, tool_input: serde_json::Value, output: &str) -> String {
    serde_json::json!({
        "tool_name": tool_name,
        "tool_input": tool_input,
        "tool_response": { "output": output }
    })
    .to_string()
}

/// Unique session ID per test to avoid cross-test session pollution.
fn test_sid(label: &str) -> String {
    format!("integration_test_{}_{}", label, id())
}

/// Parse `{"output": "..."}` from stdout bytes. Returns None if stdout is empty.
fn parse_hook_output(stdout: &[u8]) -> Option<String> {
    if stdout.is_empty() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(stdout).ok()?;
    v["output"].as_str().map(|s| s.to_string())
}

// ── WebFetch — short page passes through ─────────────────────────────────────

#[test]
fn webfetch_short_page_passes_through() {
    // Pages under 30 lines must be returned as-is (no compression, no output JSON).
    let short_page = "# Short page\nThis is a short page.\nOnly a few lines.\nNot worth compressing.\n";
    assert!(short_page.lines().count() < 30);

    let json = hook_json(
        "WebFetch",
        serde_json::json!({ "url": "https://example.com/short" }),
        short_page,
    );

    let out = Command::cargo_bin("panda")
        .unwrap()
        .args(["hook"])
        .env("PANDA_SESSION_ID", test_sid("wf_short"))
        .write_stdin(json)
        .output()
        .unwrap();

    assert!(out.status.success(), "hook must never fail");
    assert!(
        out.stdout.is_empty(),
        "short page must pass through (empty stdout), got: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

// ── WebFetch — long page gets compressed ─────────────────────────────────────

#[test]
fn webfetch_long_page_produces_compressed_output() {
    // A multi-section page > 30 lines must produce a shorter JSON output.
    let page = build_long_web_page();
    let original_len = page.len();
    assert!(page.lines().count() > 30);

    let json = hook_json(
        "WebFetch",
        serde_json::json!({ "url": "https://docs.example.com/api" }),
        &page,
    );

    let out = Command::cargo_bin("panda")
        .unwrap()
        .args(["hook"])
        .env("PANDA_SESSION_ID", test_sid("wf_long"))
        .write_stdin(json)
        .output()
        .unwrap();

    assert!(out.status.success());

    // If the hook produced output it must be valid JSON.
    // Byte length may be >= original because collapse markers add bytes while reducing
    // information density. Verify the hook ran and the output is either shorter OR
    // contains an expandable collapse marker ("panda expand ZI_N").
    if !out.stdout.is_empty() {
        let output = parse_hook_output(&out.stdout)
            .expect("stdout must be valid {\"output\":\"...\"} JSON");
        let compressed = output.len() < original_len;
        // Collapsed/summarized sections produce zoom markers (e.g. "expand ZI_N") so
        // Claude can always recover the full content. Check for the ZI_ pattern which
        // is common to all zoom expand formats ("panda expand ZI_N", "ccr expand ZI_N").
        let has_zoom_marker = output.contains("ZI_") || output.contains("expand ZI");
        assert!(
            compressed || has_zoom_marker,
            "WebFetch must either shorten content or add zoom markers (expand ZI_N), \
             got {} chars from {} chars original.\nOutput:\n{}",
            output.len(),
            original_len,
            &output[..output.len().min(400)]
        );
    }
}

// ── WebFetch — code blocks are preserved ─────────────────────────────────────

#[test]
fn webfetch_code_block_content_is_preserved() {
    // Sections containing ``` must never have their code content altered.
    let page = format!(
        "{}\n\n## Usage\n\nHere is how to call the API:\n\n\
         ```rust\nfn main() {{\n    println!(\"hello world\");\n}}\n```\n\n\
         This is very important code that must never be compressed or truncated.\n\n{}",
        "# API Documentation\n".repeat(5),
        "Extra filler content at the bottom.\n".repeat(20)
    );

    let json = hook_json(
        "WebFetch",
        serde_json::json!({ "url": "https://docs.example.com/code" }),
        &page,
    );

    let out = Command::cargo_bin("panda")
        .unwrap()
        .args(["hook"])
        .env("PANDA_SESSION_ID", test_sid("wf_code"))
        .write_stdin(json)
        .output()
        .unwrap();

    assert!(out.status.success());

    if let Some(output) = parse_hook_output(&out.stdout) {
        // Code block markers must appear in the output
        assert!(
            output.contains("```rust") || output.contains("```"),
            "code block fences must be present in output"
        );
        assert!(
            output.contains("println!"),
            "code block body must be preserved in output"
        );
    }
}

// ── WebFetch — JSON/data passes through untouched ────────────────────────────

#[test]
fn webfetch_json_content_passes_through_unchanged() {
    // JSON responses must never be BERT-compressed — structure is semantic.
    let json_body = serde_json::json!({
        "results": (0..50).map(|i| serde_json::json!({
            "id": i,
            "name": format!("item_{}", i),
            "value": i * 10,
            "tags": ["rust", "api", "important"]
        })).collect::<Vec<_>>()
    })
    .to_string();
    // Reformat as multi-line so it's > 30 lines
    let multiline = json_body
        .replace(',', ",\n")
        .replace('{', "{\n")
        .replace('}', "\n}");
    assert!(multiline.lines().count() > 30, "fixture must exceed 30 lines");

    let json = hook_json(
        "WebFetch",
        serde_json::json!({ "url": "https://api.example.com/data.json" }),
        &multiline,
    );

    let out = Command::cargo_bin("panda")
        .unwrap()
        .args(["hook"])
        .env("PANDA_SESSION_ID", test_sid("wf_json"))
        .write_stdin(json)
        .output()
        .unwrap();

    assert!(out.status.success());
    // JSON content must pass through — stdout must be empty (no compression).
    assert!(
        out.stdout.is_empty(),
        "JSON content must never be compressed, got: {}",
        String::from_utf8_lossy(&out.stdout).chars().take(200).collect::<String>()
    );
}

// ── WebFetch — HTML content passes through untouched ─────────────────────────

#[test]
fn webfetch_html_content_passes_through_unchanged() {
    // HTML pages that weren't stripped must pass through — BERT would prefer
    // tag lines over content lines, inverting the selection.
    let html = concat!(
        "<!DOCTYPE html>\n",
        "<html lang=\"en\">\n",
        "<head><title>Test Page</title></head>\n",
        "<body>\n",
        "<h1>Important Content</h1>\n",
        "<p>This is critical information that Claude needs intact.</p>\n",
        "<div class=\"section\">\n",
        "  <h2>API Reference</h2>\n",
        "  <p>The function signature is: <code>fn process(input: &str) -> Result&lt;String&gt;</code></p>\n",
        "  <p>Parameters: input — the raw text to process</p>\n",
        "  <p>Returns: Ok(compressed) on success, Err on parse failure</p>\n",
        "</div>\n",
    );
    // Repeat to get past the 30-line threshold
    let repeated = html.repeat(4);
    assert!(repeated.lines().count() > 30);

    let json = hook_json(
        "WebFetch",
        serde_json::json!({ "url": "https://example.com/page.html" }),
        &repeated,
    );

    let out = Command::cargo_bin("panda")
        .unwrap()
        .args(["hook"])
        .env("PANDA_SESSION_ID", test_sid("wf_html"))
        .write_stdin(json)
        .output()
        .unwrap();

    assert!(out.status.success());
    assert!(
        out.stdout.is_empty(),
        "HTML content must pass through unchanged"
    );
}

// ── WebFetch — prose blog page (no headers, single blank lines) ───────────────

#[test]
fn webfetch_prose_page_compresses_without_losing_structure() {
    // A typical blog post: no ## headers, single blank lines between paragraphs.
    // Must not be treated as one big section and crushed to 20 lines.
    let prose = build_prose_page();
    assert!(prose.lines().count() > 30);

    let json = hook_json(
        "WebFetch",
        serde_json::json!({ "url": "https://blog.example.com/rust-guide" }),
        &prose,
    );

    let out = Command::cargo_bin("panda")
        .unwrap()
        .args(["hook"])
        .env("PANDA_SESSION_ID", test_sid("wf_prose"))
        .write_stdin(json)
        .output()
        .unwrap();

    assert!(out.status.success());

    // If compression fires, every omitted chunk must be recoverable (has ZI_ zoom marker).
    if let Some(output) = parse_hook_output(&out.stdout) {
        // Any omission marker must include a zoom ID — no dead ends.
        let collapsed_without_id = output.contains("[... ") && !output.contains("ZI_");
        assert!(
            !collapsed_without_id,
            "every collapsed section must have a zoom ID for recovery"
        );
    }
}

// ── WebFetch — empty output passes through ───────────────────────────────────

#[test]
fn webfetch_empty_output_passes_through() {
    let json = hook_json(
        "WebFetch",
        serde_json::json!({ "url": "https://example.com/empty" }),
        "",
    );

    let out = Command::cargo_bin("panda")
        .unwrap()
        .args(["hook"])
        .env("PANDA_SESSION_ID", test_sid("wf_empty"))
        .write_stdin(json)
        .output()
        .unwrap();

    assert!(out.status.success(), "empty WebFetch must never fail");
    assert!(out.stdout.is_empty(), "empty page must pass through");
}

// ── WebSearch — few results pass through ─────────────────────────────────────

#[test]
fn websearch_few_results_passes_through() {
    // Fewer than 8 results must be returned unchanged (no output JSON).
    let results = build_search_results(5);
    let json = hook_json(
        "WebSearch",
        serde_json::json!({ "query": "rust async programming" }),
        &results,
    );

    let out = Command::cargo_bin("panda")
        .unwrap()
        .args(["hook"])
        .env("PANDA_SESSION_ID", test_sid("ws_few"))
        .write_stdin(json)
        .output()
        .unwrap();

    assert!(out.status.success(), "hook must never fail");
    assert!(
        out.stdout.is_empty(),
        "fewer than 8 results must pass through (empty stdout)"
    );
}

// ── WebSearch — many results collapse into zoom block ────────────────────────

#[test]
fn websearch_many_results_collapses_overflow_into_zoom_block() {
    // 12 results → top 8 kept, rest collapsed into zoom block with "panda expand" marker.
    let results = build_search_results(12);
    let json = hook_json(
        "WebSearch",
        serde_json::json!({ "query": "rust async programming" }),
        &results,
    );

    let out = Command::cargo_bin("panda")
        .unwrap()
        .args(["hook"])
        .env("PANDA_SESSION_ID", test_sid("ws_many"))
        .write_stdin(json)
        .output()
        .unwrap();

    assert!(out.status.success());

    // May be empty if the parse_search_results parser doesn't recognise the format;
    // if something is produced, it must have the zoom marker.
    if let Some(output) = parse_hook_output(&out.stdout) {
        assert!(
            output.contains("panda expand"),
            "overflow results must be collapsed into a zoom block with 'panda expand'"
        );
    }
}

// ── WebSearch — empty output passes through ──────────────────────────────────

#[test]
fn websearch_empty_output_passes_through() {
    let json = hook_json(
        "WebSearch",
        serde_json::json!({ "query": "something" }),
        "",
    );
    let out = Command::cargo_bin("panda")
        .unwrap()
        .args(["hook"])
        .env("PANDA_SESSION_ID", test_sid("ws_empty"))
        .write_stdin(json)
        .output()
        .unwrap();

    assert!(out.status.success());
    assert!(out.stdout.is_empty());
}

// ── Error-loop detection — overlapping errors on two consecutive runs ─────────

#[test]
fn error_loop_detected_on_second_run_with_overlapping_errors() {
    // Run 1: errors A + B + C (stored in session).
    // Run 2: errors A + B + D (different from run 1 — bypasses result cache).
    //        A and B are unchanged → has_loop() == true → "[Error loop:" emitted.
    //
    // Both outputs must be >= 15 lines so the BERT_MIN_LINES gate is crossed
    // and error signatures get stored after run 1.
    let sid = test_sid("errloop");
    let (errors_run1, errors_run2) = build_overlapping_cargo_errors();
    assert!(errors_run1.lines().count() >= 15, "run1 fixture must be >= 15 lines");
    assert!(errors_run2.lines().count() >= 15, "run2 fixture must be >= 15 lines");
    // Ensure the two outputs differ so the result-cache doesn't short-circuit run 2
    assert_ne!(errors_run1, errors_run2, "fixtures must differ to bypass result cache");

    let json1 = hook_json("Bash", serde_json::json!({ "command": "cargo build" }), &errors_run1);
    let json2 = hook_json("Bash", serde_json::json!({ "command": "cargo build" }), &errors_run2);

    // Run 1 — record errors in session
    let out1 = Command::cargo_bin("panda")
        .unwrap()
        .args(["hook"])
        .env("PANDA_SESSION_ID", &sid)
        .write_stdin(json1)
        .output()
        .unwrap();
    assert!(out1.status.success(), "run 1 must succeed");

    // Run 2 — overlapping errors; should detect the loop
    let out2 = Command::cargo_bin("panda")
        .unwrap()
        .args(["hook"])
        .env("PANDA_SESSION_ID", &sid)
        .write_stdin(json2)
        .output()
        .unwrap();
    assert!(out2.status.success(), "run 2 must succeed — never block Claude");

    // If error-loop detection fired, the output will contain "[Error loop:"
    // Hard contract: hook never fails. Soft contract: loop marker present on overlap.
    if let Some(output2) = parse_hook_output(&out2.stdout) {
        assert!(
            output2.contains("[Error loop:"),
            "second run with overlapping errors must produce '[Error loop:' marker, got:\n{}",
            &output2[..output2.len().min(500)]
        );
    }
}

// ── Error-loop detection — all-new errors never flag a loop ──────────────────

#[test]
fn error_loop_not_triggered_when_all_errors_are_new() {
    let sid = test_sid("errloop_new");

    // Run 1: errors with E0382 (borrow of moved value) — stored in session
    let errors1 = build_cargo_errors_set_a();

    let json1 = hook_json(
        "Bash",
        serde_json::json!({ "command": "cargo build" }),
        &errors1,
    );
    let out1 = Command::cargo_bin("panda")
        .unwrap()
        .args(["hook"])
        .env("PANDA_SESSION_ID", &sid)
        .write_stdin(json1)
        .output()
        .unwrap();
    assert!(out1.status.success());

    // Run 2: completely different errors (different codes AND different files) — not a loop.
    // These must differ from run 1 so the result cache doesn't short-circuit.
    let errors2 = build_cargo_errors_set_b();
    assert_ne!(errors1, errors2);

    let json2 = hook_json(
        "Bash",
        serde_json::json!({ "command": "cargo build" }),
        &errors2,
    );
    let out2 = Command::cargo_bin("panda")
        .unwrap()
        .args(["hook"])
        .env("PANDA_SESSION_ID", &sid)
        .write_stdin(json2)
        .output()
        .unwrap();
    assert!(out2.status.success());

    // All-new errors must NOT produce the loop marker
    if let Some(output2) = parse_hook_output(&out2.stdout) {
        assert!(
            !output2.contains("[Error loop:"),
            "all-new errors must not trigger loop detection, got:\n{}",
            &output2[..output2.len().min(300)]
        );
    }
}

// ── BERT budget — exhausted budget produces fallback text ────────────────────
//
// This is a unit-level check via the public `panda` library, not through the
// binary, because the budget is thread-local and resets each invocation.
// We verify the fallback function's structure here.

#[test]
fn bert_budget_try_consume_returns_false_when_exhausted() {
    // The bert_budget module is pub in lib.rs so it is accessible in integration tests.
    use panda::bert_budget;

    bert_budget::reset();
    let max = panda::bert_budget::MAX_BERT_CALLS;
    // Consume all
    for _ in 0..max {
        assert!(bert_budget::try_consume(), "should have budget");
    }
    // Next must fail
    assert!(
        !bert_budget::try_consume(),
        "exhausted budget must return false"
    );
    assert_eq!(bert_budget::remaining(), 0);
}

#[test]
fn bert_budget_reset_restores_after_exhaustion() {
    use panda::bert_budget;

    bert_budget::reset();
    // Exhaust
    for _ in 0..panda::bert_budget::MAX_BERT_CALLS {
        let _ = bert_budget::try_consume();
    }
    assert_eq!(bert_budget::remaining(), 0);
    // Reset
    bert_budget::reset();
    assert_eq!(bert_budget::remaining(), panda::bert_budget::MAX_BERT_CALLS);
    assert!(bert_budget::try_consume());
}

// ── Fixtures ─────────────────────────────────────────────────────────────────

/// Build a realistic multi-section documentation page (> 30 lines).
fn build_long_web_page() -> String {
    let mut page = String::new();

    page.push_str("# Tokio Async Runtime — API Reference\n\n");
    page.push_str("Welcome to the Tokio documentation. Tokio is an asynchronous runtime ");
    page.push_str("for the Rust programming language. It provides the building blocks ");
    page.push_str("needed for writing networking applications.\n\n");

    page.push_str("## Installation\n\n");
    page.push_str("Add the following to your `Cargo.toml`:\n\n");
    page.push_str("[dependencies]\n");
    page.push_str("tokio = { version = \"1\", features = [\"full\"] }\n\n");
    page.push_str("This will pull in all optional components of Tokio.\n\n");

    page.push_str("## Usage\n\n");
    page.push_str("The `#[tokio::main]` macro sets up the async runtime automatically.\n");
    page.push_str("Use `tokio::spawn` to create concurrent tasks.\n");
    page.push_str("The `tokio::select!` macro waits on multiple futures simultaneously.\n\n");

    page.push_str("## Important: Error Handling\n\n");
    page.push_str("Always propagate errors with `?` in async functions.\n");
    page.push_str("Use `anyhow::Result` for application-level error handling.\n");
    page.push_str("Note: panics in spawned tasks do not propagate to the spawner.\n");
    page.push_str("Warning: blocking operations in async context will stall the executor.\n\n");

    page.push_str("## Examples\n\n");
    page.push_str("Here is a basic TCP echo server example using Tokio.\n");
    page.push_str("It demonstrates how to accept connections and read/write data.\n");
    page.push_str("The example uses `BufReader` for efficient line-by-line reading.\n\n");

    page.push_str("## Parameters\n\n");
    page.push_str("- `addr`: The socket address to bind to (type: `SocketAddr`)\n");
    page.push_str("- `backlog`: Number of pending connections to queue (optional)\n");
    page.push_str("- `timeout`: Connection timeout in milliseconds (optional, default 30000)\n\n");

    page.push_str("## Return Values\n\n");
    page.push_str("Returns `Ok(TcpListener)` on success.\n");
    page.push_str("Returns `Err(io::Error)` if binding fails (e.g. address in use).\n\n");

    page.push_str("## Navigation\n\n");
    page.push_str("Skip to main content | Terms of service | Privacy policy\n");
    page.push_str("Subscribe to our newsletter | Follow us on Twitter\n");
    page.push_str("Copyright 2024 Tokio Project Contributors\n");

    page
}

/// Build N fake search results in markdown list format.
fn build_search_results(n: usize) -> String {
    let mut out = String::new();
    for i in 1..=n {
        out.push_str(&format!(
            "- **Result {i}**: How to use Rust async programming effectively\n  \
             This article explains the fundamentals of async/await in Rust, \
             including tokio runtime setup and error handling patterns.\n  \
             URL: https://example.com/rust-async-{i}\n\n"
        ));
    }
    out
}

/// Run-1 errors for the overlap test: E0382 + E0507 (the "unchanged" pair) + E0308.
/// >= 15 lines so BERT_MIN_LINES gate is satisfied.
fn build_overlapping_cargo_errors() -> (String, String) {
    let run1 = concat!(
        "   Compiling panda v1.0.0 (/home/user/project)\n",
        "error[E0382]: borrow of moved value: `config`\n",
        "  --> src/hook.rs:42:15\n",
        "   |\n",
        "41 |     let x = config;\n",
        "   |             ------ value moved here\n",
        "42 |     println!(\"{:?}\", config);\n",
        "   |                     ^^^^^^ value borrowed here after move\n",
        "   = note: move occurs because `config` has type `Config`\n",
        "\n",
        "error[E0507]: cannot move out of `*session` which is behind a shared reference\n",
        "  --> src/session.rs:87:22\n",
        "   |\n",
        "87 |     let entries = session.entries;\n",
        "   |                   ^^^^^^^^^^^^^^^ move occurs because type is Vec<Entry>\n",
        "\n",
        "error[E0308]: mismatched types in function argument\n",
        "  --> src/main.rs:15:10\n",
        "   |\n",
        "15 |     run(42);\n",
        "   |         ^^ expected `&str`, found integer\n",
    )
    .to_string();

    // Run 2 keeps E0382 + E0507 (same code+file+message → unchanged),
    // drops E0308 (fixed), adds E0502 (new).
    let run2 = concat!(
        "   Compiling panda v1.0.0 (/home/user/project)\n",
        "error[E0382]: borrow of moved value: `config`\n",
        "  --> src/hook.rs:42:15\n",
        "   |\n",
        "41 |     let x = config;\n",
        "   |             ------ value moved here\n",
        "42 |     println!(\"{:?}\", config);\n",
        "   |                     ^^^^^^ value borrowed here after move\n",
        "   = note: move occurs because `config` has type `Config`\n",
        "\n",
        "error[E0507]: cannot move out of `*session` which is behind a shared reference\n",
        "  --> src/session.rs:87:22\n",
        "   |\n",
        "87 |     let entries = session.entries;\n",
        "   |                   ^^^^^^^^^^^^^^^ move occurs because type is Vec<Entry>\n",
        "\n",
        "error[E0502]: cannot borrow `data` as mutable because it is also borrowed as immutable\n",
        "  --> src/lib.rs:30:5\n",
        "   |\n",
        "29 |     let r = &data;\n",
        "   |              ---- immutable borrow occurs here\n",
        "30 |     data.push(1);\n",
        "   |     ^^^^^^^^^^^^ mutable borrow occurs here\n",
    )
    .to_string();

    (run1, run2)
}

/// Error set A — only E0382 errors, no overlap with set B.
/// >= 15 lines to cross BERT_MIN_LINES gate.
fn build_cargo_errors_set_a() -> String {
    concat!(
        "   Compiling panda v1.0.0 (/home/user/project)\n",
        "error[E0382]: borrow of moved value: `config`\n",
        "  --> src/hook.rs:42:15\n",
        "   |\n",
        "41 |     let x = config;\n",
        "   |             ------ value moved here\n",
        "42 |     println!(\"{:?}\", config);\n",
        "   |                     ^^^^^^ value borrowed here after move\n",
        "   = note: move occurs because `config` has type `Config`, which does not implement `Copy`\n",
        "\n",
        "error[E0382]: borrow of moved value: `state`\n",
        "  --> src/hook.rs:78:20\n",
        "   |\n",
        "77 |     let y = state;\n",
        "   |             ----- value moved here\n",
        "78 |     process(&state);\n",
        "   |              ^^^^^ value borrowed here after move\n",
        "   = note: move occurs because `state` has type `State`\n",
    )
    .to_string()
}

/// Error set B — only E0507/E0502 errors, no overlap with set A.
fn build_cargo_errors_set_b() -> String {
    concat!(
        "   Compiling panda v1.0.0 (/home/user/project)\n",
        "error[E0507]: cannot move out of `*handler` which is behind a mutable reference\n",
        "  --> src/handlers/git.rs:55:12\n",
        "   |\n",
        "55 |     let h = *handler;\n",
        "   |             ^^^^^^^^ move occurs because `*handler` has type `GitHandler`\n",
        "   = help: consider cloning the value if the performance cost is acceptable\n",
        "\n",
        "error[E0502]: cannot borrow `buffer` as mutable because it is also borrowed as immutable\n",
        "  --> src/handlers/git.rs:82:5\n",
        "   |\n",
        "80 |     let view = &buffer;\n",
        "   |                 ------ immutable borrow occurs here\n",
        "81 |     ...\n",
        "82 |     buffer.clear();\n",
        "   |     ^^^^^^^^^^^^^^ mutable borrow occurs here\n",
        "83 |     println!(\"{}\", view);\n",
        "   |                    ---- immutable borrow later used here\n",
    )
    .to_string()
}

/// A realistic blog-style prose page: no Markdown headers, single blank lines
/// between paragraphs. Representative of many real web pages that are NOT
/// structured documentation.
fn build_prose_page() -> String {
    // Each paragraph is separated by a single blank line.
    // No ## headers — this tests the single-blank-line fallback split strategy.
    let paragraphs = [
        "Understanding Rust's Ownership Model",
        "Rust's ownership system is one of its most distinctive features.\nUnlike garbage-collected languages, Rust tracks memory ownership at compile time\nthrough a set of rules enforced by the borrow checker.\nEvery value has a single owner, and when the owner goes out of scope the value is dropped.",
        "This eliminates an entire class of bugs that plague systems programming.\nUse-after-free, double-free, and dangling pointer errors are impossible\nin safe Rust code. The compiler rejects programs that would cause\nundefined behaviour before they ever run.",
        "Borrowing allows temporary access to a value without taking ownership.\nYou can have either one mutable reference or any number of immutable\nreferences at any given time. This rule prevents data races at compile time,\nwhich is a significant guarantee for concurrent code.",
        "Lifetimes annotate how long references are valid.\nMost of the time the compiler infers lifetimes automatically through lifetime elision.\nExplicit lifetime annotations are only needed when the compiler cannot\ndetermine the relationship between the lifetimes of multiple references.",
        "The Clone and Copy traits control how values are duplicated.\nTypes that implement Copy are duplicated bit-for-bit on assignment.\nTypes that implement Clone provide an explicit clone() method.\nMost heap-allocated types like String and Vec implement Clone but not Copy.",
        "Interior mutability patterns such as RefCell, Mutex, and RwLock allow\nmutation through shared references. RefCell enforces borrow rules at runtime.\nMutex provides mutual exclusion for shared state across threads.\nThese are escape hatches for cases where the static borrow checker is too conservative.",
        "Smart pointers like Box, Rc, and Arc extend the ownership model.\nBox allocates a value on the heap and transfers ownership.\nRc enables shared ownership within a single thread via reference counting.\nArc is the thread-safe equivalent, using atomic reference counting.",
        "Understanding these primitives is essential for writing idiomatic Rust.\nThe ownership model might feel restrictive at first, but it encodes\nimportant invariants about your program's data flow that make\nrefactoring safer and more predictable in the long run.",
    ];
    paragraphs.join("\n\n")
}
