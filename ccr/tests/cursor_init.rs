/// Integration tests for `panda init --agent cursor` / `panda init --agent cursor --uninstall`.
///
/// Each test overrides $HOME with a temporary directory so nothing touches the
/// real ~/.cursor.  No new dev-dependencies are needed: assert_cmd, tempfile,
/// predicates and serde_json are already in [dev-dependencies].
use assert_cmd::Command;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

fn ccr() -> Command {
    Command::cargo_bin("panda").unwrap()
}

fn cursor_hooks_json(home: &TempDir) -> PathBuf {
    home.path().join(".cursor").join("hooks.json")
}

fn cursor_script(home: &TempDir) -> PathBuf {
    home.path().join(".cursor").join("hooks").join("panda-rewrite.sh")
}

/// Run `panda init --agent cursor` with a fake home directory.
/// Pre-creates ~/.cursor so the "Cursor not installed" early-exit doesn't trigger.
fn run_cursor_init(home: &TempDir) {
    fs::create_dir_all(home.path().join(".cursor")).unwrap();
    ccr()
        .args(["init", "--agent", "cursor"])
        .env("HOME", home.path())
        .assert()
        .success();
}

// ── 1. Script created ─────────────────────────────────────────────────────────

#[test]
fn test_cursor_init_creates_script() {
    let home = TempDir::new().unwrap();
    run_cursor_init(&home);
    assert!(cursor_script(&home).exists(), "panda-rewrite.sh should exist");
}

// ── 2. Script is executable ───────────────────────────────────────────────────

#[cfg(unix)]
#[test]
fn test_cursor_init_script_is_executable() {
    use std::os::unix::fs::PermissionsExt;
    let home = TempDir::new().unwrap();
    run_cursor_init(&home);
    let mode = fs::metadata(cursor_script(&home)).unwrap().permissions().mode();
    assert!(mode & 0o111 != 0, "script should be executable (mode {:o})", mode);
}

// ── 3. hooks.json structure ────────────────────────────────────────────────────

#[test]
fn test_cursor_init_creates_hooks_json() {
    let home = TempDir::new().unwrap();
    run_cursor_init(&home);

    let content = fs::read_to_string(cursor_hooks_json(&home)).unwrap();
    let root: serde_json::Value = serde_json::from_str(&content).unwrap();

    assert_eq!(root["version"], 1, "hooks.json should have version:1");

    let pre = root["hooks"]["preToolUse"].as_array().unwrap();
    let has_panda = pre.iter().any(|e| {
        e["command"].as_str().unwrap_or("").contains("panda-rewrite.sh")
            && e["matcher"].as_str().unwrap_or("") == "Shell"
    });
    assert!(has_panda, "preToolUse should have a Shell entry pointing to panda-rewrite.sh");
}

// ── 4. PostToolUse entries ─────────────────────────────────────────────────────

#[test]
fn test_cursor_init_adds_posttooluse_entries() {
    let home = TempDir::new().unwrap();
    run_cursor_init(&home);

    let content = fs::read_to_string(cursor_hooks_json(&home)).unwrap();
    let root: serde_json::Value = serde_json::from_str(&content).unwrap();

    let post = root["hooks"]["postToolUse"].as_array().unwrap();
    let matchers: Vec<&str> = post
        .iter()
        .filter(|e| {
            let cmd = e["command"].as_str().unwrap_or("");
            cmd.contains("panda hook") && cmd.contains("PANDA_AGENT=cursor")
        })
        .filter_map(|e| e["matcher"].as_str())
        .collect();

    assert!(matchers.contains(&"Bash"), "postToolUse Bash entry missing");
    assert!(matchers.contains(&"Read"), "postToolUse Read entry missing");
    assert!(matchers.contains(&"Glob"), "postToolUse Glob entry missing");
}

// ── 5. Idempotent ─────────────────────────────────────────────────────────────

#[test]
fn test_cursor_init_idempotent() {
    let home = TempDir::new().unwrap();
    run_cursor_init(&home);
    run_cursor_init(&home); // second call

    let content = fs::read_to_string(cursor_hooks_json(&home)).unwrap();
    let root: serde_json::Value = serde_json::from_str(&content).unwrap();

    let pre = root["hooks"]["preToolUse"].as_array().unwrap();
    let panda_count = pre
        .iter()
        .filter(|e| e["command"].as_str().unwrap_or("").contains("panda-rewrite.sh"))
        .count();
    assert_eq!(panda_count, 1, "preToolUse should have exactly one PandaFilter entry after two inits");
}

// ── 6. Preserves existing entries ─────────────────────────────────────────────

#[test]
fn test_cursor_init_preserves_existing_entries() {
    let home = TempDir::new().unwrap();

    // Pre-populate hooks.json with a non-PandaFilter entry
    let cursor_dir = home.path().join(".cursor");
    fs::create_dir_all(&cursor_dir).unwrap();
    let existing = serde_json::json!({
        "version": 1,
        "hooks": {
            "preToolUse": [
                {"command": "./hooks/other-tool.sh", "matcher": "Shell"}
            ]
        }
    });
    fs::write(cursor_dir.join("hooks.json"), serde_json::to_string_pretty(&existing).unwrap()).unwrap();

    run_cursor_init(&home);

    let content = fs::read_to_string(cursor_hooks_json(&home)).unwrap();
    let root: serde_json::Value = serde_json::from_str(&content).unwrap();

    let pre = root["hooks"]["preToolUse"].as_array().unwrap();
    let has_other = pre.iter().any(|e| {
        e["command"].as_str().unwrap_or("").contains("other-tool.sh")
    });
    assert!(has_other, "pre-existing entry should be preserved");
}

// ── 7. Uninstall removes script ────────────────────────────────────────────────

#[test]
fn test_cursor_uninstall_removes_script() {
    let home = TempDir::new().unwrap();
    run_cursor_init(&home);
    assert!(cursor_script(&home).exists());

    ccr()
        .args(["init", "--agent", "cursor", "--uninstall"])
        .env("HOME", home.path())
        .assert()
        .success();

    assert!(!cursor_script(&home).exists(), "script should be removed after uninstall");

    let hash_file = home.path().join(".cursor").join("hooks").join(".panda-hook.sha256");
    assert!(!hash_file.exists(), "hash file should be removed after uninstall");
}

// ── 8. Uninstall strips hooks.json ─────────────────────────────────────────────

#[test]
fn test_cursor_uninstall_strips_hooks_json() {
    let home = TempDir::new().unwrap();
    run_cursor_init(&home);

    ccr()
        .args(["init", "--agent", "cursor", "--uninstall"])
        .env("HOME", home.path())
        .assert()
        .success();

    let content = fs::read_to_string(cursor_hooks_json(&home)).unwrap();
    let root: serde_json::Value = serde_json::from_str(&content).unwrap();

    for event in &["preToolUse", "postToolUse"] {
        if let Some(arr) = root["hooks"][event].as_array() {
            let has_panda = arr.iter().any(|e| e["command"].as_str().unwrap_or("").contains("panda"));
            assert!(!has_panda, "{} should have no PandaFilter entries after uninstall", event);
        }
    }
}

// ── 9. Default `panda init` does not touch ~/.cursor ────────────────────────────

#[test]
fn test_claude_init_unaffected() {
    let home = TempDir::new().unwrap();

    ccr()
        .args(["init"])
        .env("HOME", home.path())
        .assert()
        .success();

    assert!(
        !home.path().join(".cursor").exists(),
        "panda init (claude) should not create ~/.cursor"
    );
}

// ── 10b. init --agent cursor skips gracefully when ~/.cursor absent ───────────

#[test]
fn test_cursor_init_no_cursor_installed() {
    let home = TempDir::new().unwrap();
    // Do NOT create ~/.cursor — simulates a machine without Cursor

    let output = ccr()
        .args(["init", "--agent", "cursor"])
        .env("HOME", home.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let text = String::from_utf8_lossy(&output);
    assert!(text.contains("Cursor not found"), "should print skip message");
    assert!(!home.path().join(".cursor").exists(), "should not create ~/.cursor");
}

// ── 11. `panda rewrite unknown-tool` exits 1 ────────────────────────────────────

#[test]
fn test_cursor_no_rewrite_exits_nonzero() {
    // This simulates the `|| { echo '{}'; exit 0; }` path in the Cursor hook script.
    // `panda rewrite` exits 1 when no rewrite applies; the hook script then returns `{}`.
    ccr()
        .args(["rewrite", "totally-unknown-tool-xyz-12345"])
        .assert()
        .failure();
}
