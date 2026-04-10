use assert_cmd::Command;

#[test]
fn hook_mode_parses_claude_code_json() {
    let json = r#"{"tool_name":"Bash","tool_input":{"command":"cargo build"},"tool_response":{"output":"   Compiling foo v1.0\n"}}"#;
    let mut cmd = Command::cargo_bin("panda").unwrap();
    cmd.args(["hook"])
        .write_stdin(json)
        .assert()
        .success();
}

#[test]
fn hook_mode_command_unchanged_if_no_filter_match() {
    let json = r#"{"tool_name":"Bash","tool_input":{"command":"echo hi"},"tool_response":{"output":"hi\n"}}"#;
    let mut cmd = Command::cargo_bin("panda").unwrap();
    cmd.args(["hook"])
        .write_stdin(json)
        .assert()
        .success();
}

#[test]
fn hook_mode_malformed_json_exits_cleanly() {
    let mut cmd = Command::cargo_bin("panda").unwrap();
    cmd.args(["hook"])
        .write_stdin("not valid json at all !!!")
        .assert()
        .success(); // must never exit non-zero — Claude Code must not be blocked
}
