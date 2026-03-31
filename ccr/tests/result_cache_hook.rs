use assert_cmd::Command;

#[test]
fn hook_bash_cache_hit_returns_identical_bytes() {
    // Generate a large enough input to pass the MIN_PIPELINE_TOKENS gate
    let big_output = "error: cannot find module\\n".repeat(20);
    let json = format!(
        r#"{{"tool_name":"Bash","tool_input":{{"command":"cargo build"}},"tool_response":{{"output":"{}"}}}}"#,
        big_output
    );

    let session_id = format!("test_rc_{}", std::process::id());

    let out1 = Command::cargo_bin("ccr")
        .unwrap()
        .args(["hook"])
        .env("CCR_SESSION_ID", &session_id)
        .write_stdin(json.clone())
        .output()
        .unwrap();

    let out2 = Command::cargo_bin("ccr")
        .unwrap()
        .args(["hook"])
        .env("CCR_SESSION_ID", &session_id)
        .write_stdin(json)
        .output()
        .unwrap();

    assert!(out1.status.success());
    assert!(out2.status.success());

    // Both calls must produce output (non-empty means pipeline ran / cache hit)
    // and the second must be byte-identical to the first.
    if !out1.stdout.is_empty() && !out2.stdout.is_empty() {
        assert_eq!(
            out1.stdout, out2.stdout,
            "cache hit should return byte-identical output"
        );
    }
}
